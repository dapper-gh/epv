use crate::{
    config::{Config, Users},
    util,
};
use async_imap::{imap_proto::Address, Client as ImapClient};
use futures::StreamExt;
use futures_rustls::pki_types::ServerName;
use futures_rustls::rustls::{ClientConfig, RootCertStore};
use futures_rustls::TlsConnector;
use itertools::Itertools;
use sqlx::{Pool, Sqlite};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;
use tiny_keccak::{Hasher, Sha3};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time;
use tokio_util::compat::TokioAsyncReadCompatExt;

fn address_to_string(address: &Address) -> String {
    format!(
        "{}@{}",
        address
            .mailbox
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or(Cow::Borrowed("")),
        address
            .host
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or(Cow::Borrowed(""))
    )
}

pub async fn perform(config: Arc<Config>, pool: Pool<Sqlite>) {
    let tcp = TcpStream::connect((config.imap.server.as_str(), config.imap.port))
        .await
        .expect("Could not establish TCP connection");

    let mut root_store = RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().expect("Unable to load native certs") {
        root_store.add(cert).expect("Unable to add root cert");
    }

    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls_connector = TlsConnector::from(Arc::new(tls_config));
    let tls_stream = tls_connector
        .connect(
            ServerName::try_from(config.imap.server.clone()).expect("Invalid domain"),
            tcp.compat(),
        )
        .await
        .expect("Unable to establish TLS connection");

    let mut imap = ImapClient::new(tls_stream);

    let _ = imap.read_response().await.expect("Could not read greeting");

    let mut session = imap
        .login(config.imap.username.as_str(), config.imap.password.as_str())
        .await
        .expect("Could not log in");
    let _ = session
        .select("EPV")
        .await
        .expect("Could not select mailbox");

    loop {
        time::sleep(Duration::from_secs(5)).await;

        let seq_list = match session.search("ALL").await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("IMAP search error: {:#?}", e);
                continue;
            }
        };

        let seq_list_str = match seq_list.len() {
            0 => continue,
            1 => seq_list
                .into_iter()
                .next()
                .expect("Just checked len, but no first element")
                .to_string(),
            _ => seq_list.into_iter().join(","),
        };

        let mut emails = match session.fetch(seq_list_str, "(ENVELOPE RFC822)").await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("IMAP fetch error: {:#?}", e);
                continue;
            }
        };

        let mut moveable_seqs = vec![];

        while let Some(email_res) = emails.next().await {
            let email = match email_res {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("IMAP individual fetch error: {:#?}", e);
                    continue;
                }
            };

            let Some(envelope) = email.envelope() else {
                eprintln!("IMAP no envelope");
                continue;
            };

            let Some(to) = &envelope.to else {
                eprintln!("IMAP no to address");
                continue;
            };

            let Some((matching_user, to_address_string)) = (match &config.users {
                Users::Many(users) => to.iter().find_map(|to_address| {
                    if let Some(host) = &to_address.host {
                        if host.len() >= config.imap.postfix.len() {
                            let (user, postfix) =
                                host.split_at(host.len() - config.imap.postfix.len());
                            if postfix == config.imap.postfix.as_bytes() {
                                return users
                                    .iter()
                                    .find(|user_full| {
                                        user_full.username.as_bytes() == user
                                    })
                                    .map(|val| (val, address_to_string(to_address)));
                            }
                        }
                    }

                    None
                }),
                Users::Single(user) => to.iter().next().map(|to_address| {
                    (user, address_to_string(to_address))
                }),
            }) else {
                eprintln!("IMAP no matching user");
                continue;
            };

            let Some(from_address_string) = envelope
                .from
                .as_ref()
                .and_then(|froms| froms.get(0))
                .map(address_to_string)
            else {
                eprintln!("IMAP no from address");
                continue;
            };

            let Some(body_bytes) = email.body() else {
                eprintln!("IMAP no email body");
                continue;
            };

            let parsed = match mailparse::parse_mail(body_bytes) {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("IMAP mail parse error: {:#?}", e);
                    continue;
                }
            };

            let Some(subject) = parsed.headers.iter().find_map(|header| {
                if header.get_key_ref() == "Subject" {
                    Some(header.get_value())
                } else {
                    None
                }
            }) else {
                eprintln!("IMAP subject None");
                continue;
            };

            let Some(html) =
                util::traverse_mail(&parsed, &mut |mail| &mail.ctype.mimetype == "text/html")
            else {
                eprintln!("IMAP mail no body");
                continue;
            };

            let html_body = match html.get_body() {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("IMAP mail parse body error: {:#?}", e);
                    continue;
                }
            };

            let mut sha3 = Sha3::v256();
            let mut output = [0; 32];
            sha3.update(body_bytes);
            sha3.finalize(&mut output);
            let id = hex::encode(&output[0..16]);

            match sqlx::query!(r#"SELECT 1 as existence FROM emails WHERE id = $1"#, id)
                .fetch_optional(&pool)
                .await
            {
                Ok(Some(_)) => {
                    moveable_seqs.push(email.message);
                    continue;
                }
                Err(e) => {
                    eprintln!("IMAP check existence error: {:#?}", e);
                    continue;
                }
                _ => {}
            }

            let file_name = format!("{}/{}.html", matching_user.username, id);

            let mut html_file = match util::open_parents(
                OpenOptions::new().write(true).truncate(true).create(true),
                format!("{}/{}", config.storage.file_root, file_name),
            )
            .await
            {
                Ok(file) => file,
                Err(e) => {
                    eprintln!("IMAP could not open file: {:#?}", e);
                    continue;
                }
            };

            if let Err(e) = html_file.write(html_body.as_bytes()).await {
                eprintln!("IMAP file write error: {:#?}", e);
                continue;
            }

            let now = util::unix_ms();

            if let Err(e) = sqlx::query!(
                r#"INSERT INTO emails (id, html, user, registered, subject, from_addr, to_addr)
                           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
                id,
                file_name,
                matching_user.username,
                now,
                subject,
                from_address_string,
                to_address_string
            )
            .execute(&pool)
            .await
            {
                eprintln!("IMAP insert error: {:#?}", e);
            }

            moveable_seqs.push(email.message);
        }

        drop(emails);

        if !moveable_seqs.is_empty() {
            if let Err(e) = session
                .mv(
                    moveable_seqs.into_iter().map(|n| n.to_string()).join(","),
                    "EPV-READ",
                )
                .await
            {
                eprintln!("IMAP move error: {:#?}", e);
            }
        }
    }
}
