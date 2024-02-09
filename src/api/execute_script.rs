use crate::{
    rocket_types::{AuthorizedUser, Error, FlexibleFormat, Ratelimit},
    sql::Email,
    ManagedConfig, ManagedPool, ManagedUrlCache,
};
use futures::Future;
use itertools::Itertools;
use regex::Regex;
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client as HttpClient,
};
use rocket::{serde::json::Json, State};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use tokio::{fs, sync::mpsc};
use url::Url;

#[derive(Debug, Deserialize, Clone)]
pub struct Script {
    actions: Vec<Action>,
}

#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(tag = "name", content = "arguments")]
pub enum Action {
    EmailToHtml,
    EmailFilterRegex(EmailAttribute, String),
    EmailGetAttr(EmailAttribute),

    HtmlInnerText,
    HtmlOuterHtml,
    HtmlInnerHtml,
    HtmlGetAttr(String),
    HtmlSelectCss(String),
    HtmlFilterCss(String),

    TextMatchRegex(String, String),
    TextFilterRegex(String),
    TextToHtml,
    TextToUrl,

    UrlToText,
    UrlFollowRedirect,
    UrlGetQuery(String),
    UrlGetSegment(i8),

    ArraySelectNth(usize),

    PairGetLeft,
    PairGetRight,
    PairZipTogether,
    PairDistributeLeft,
    PairRightLeft,

    Macro(String),

    Or(Vec<Action>, Vec<Action>),
    Pair(Vec<Action>, Vec<Action>),
    Filter(Vec<Action>),
}

#[derive(Debug, Deserialize, Clone, Copy, Serialize)]
pub enum EmailAttribute {
    Id,
    FromAddress,
    ToAddress,
    Subject,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", content = "value")]
pub enum SerdeElement {
    Html(Arc<str>),
    Text(Arc<str>),
    Email(String),
    Url(String),
    Pair(Vec<SerdeElement>, Vec<SerdeElement>),
}

#[derive(Debug, Clone)]
enum Element {
    Html(Arc<str>),
    Text(Arc<str>),
    Email(Arc<Email>),
    Url(Url),
    Pair(Vec<Element>, Vec<Element>),
}
impl From<Element> for SerdeElement {
    fn from(value: Element) -> Self {
        match value {
            Element::Html(el) => SerdeElement::Html(el),
            Element::Text(str) => SerdeElement::Text(str),
            Element::Email(eml) => SerdeElement::Email(eml.id.to_owned()),
            Element::Url(url) => SerdeElement::Url(url.to_string()),
            Element::Pair(elements1, elements2) => SerdeElement::Pair(
                elements1.into_iter().map(SerdeElement::from).collect(),
                elements2.into_iter().map(SerdeElement::from).collect(),
            ),
        }
    }
}

trait FragmentRoot {
    fn fragment_root(&self) -> Option<ElementRef<'_>>;
}
impl FragmentRoot for Html {
    fn fragment_root(&self) -> Option<ElementRef<'_>> {
        self.select(
            &Selector::parse(":not(head, body, html)")
                .expect("fragment_root: invalid premade selector"),
        )
        .next()
    }
}

enum ActionMessage {
    Done,
    Error(Error),
    Element(Element),
}

fn exec_action(
    action: Arc<Action>,
    element_index: usize,
    element: Element,
    channel: mpsc::Sender<ActionMessage>,
    config: ManagedConfig,
    url_cache: ManagedUrlCache,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        let mut msgs_to_send = vec![];
        let mut error = None;

        match (&*action, element) {
            (Action::EmailToHtml, Element::Email(email)) => {
                let html_string = match fs::read_to_string(format!(
                    "{}/{}",
                    config.storage.file_root, email.html
                ))
                .await
                {
                    Ok(x) => x,
                    Err(e) => {
                        eprintln!("/emails/execute-script file read error: {:#?}", e);
                        let _ = channel
                            .send(ActionMessage::Error(Error::InternalError))
                            .await;
                        return;
                    }
                };

                let _ = channel
                    .send(ActionMessage::Element(Element::Html(html_string.into())))
                    .await;
            }
            (Action::HtmlSelectCss(selector_str), Element::Html(html_string)) => {
                match Selector::parse(&selector_str) {
                    Ok(selector) => {
                        let html_element = Html::parse_fragment(&html_string);

                        msgs_to_send.extend(
                            html_element
                                .select(&selector)
                                .map(|el| ActionMessage::Element(Element::Html(el.html().into()))),
                        );
                    }
                    Err(_) => {
                        error = Some(ActionMessage::Error(Error::InvalidInput(
                            selector_str.to_owned(),
                        )));
                    }
                };
            }
            (Action::HtmlFilterCss(selector_str), Element::Html(html_string)) => {
                match Selector::parse(&selector_str) {
                    Ok(selector) => {
                        let html_element = Html::parse_fragment(&html_string);

                        if html_element.select(&selector).count() != 0 {
                            msgs_to_send.push(ActionMessage::Element(Element::Html(html_string)));
                        }
                    }
                    Err(_) => {
                        error = Some(ActionMessage::Error(Error::InvalidInput(
                            selector_str.to_owned(),
                        )));
                    }
                };
            }
            (Action::HtmlInnerText, Element::Html(html_string)) => {
                let html_element = Html::parse_fragment(&html_string);
                msgs_to_send.extend(
                    html_element.fragment_root().map(|el| {
                        ActionMessage::Element(Element::Text(el.text().join(" ").into()))
                    }),
                );
            }
            (Action::HtmlOuterHtml, Element::Html(html_string)) => {
                let _ = channel
                    .send(ActionMessage::Element(Element::Text(html_string)))
                    .await;
            }
            (Action::HtmlInnerHtml, Element::Html(html_string)) => {
                let html_element = Html::parse_fragment(&html_string);
                msgs_to_send.extend(
                    html_element
                        .fragment_root()
                        .map(|el| ActionMessage::Element(Element::Text(el.inner_html().into()))),
                );
            }
            (Action::TextMatchRegex(regex_string, replacement), Element::Text(string)) => {
                let regex = match Regex::new(regex_string) {
                    Ok(x) => x,
                    Err(_e) => {
                        let _ = channel
                            .send(ActionMessage::Error(Error::InvalidInput(
                                regex_string.to_owned(),
                            )))
                            .await;
                        return;
                    }
                };

                for cap in regex.captures_iter(&string) {
                    let mut destination = String::new();
                    cap.expand(replacement, &mut destination);
                    let _ = channel
                        .send(ActionMessage::Element(Element::Text(destination.into())))
                        .await;
                }
            }
            (Action::TextFilterRegex(regex_string), Element::Text(string)) => {
                let regex = match Regex::new(regex_string) {
                    Ok(x) => x,
                    Err(_e) => {
                        let _ = channel
                            .send(ActionMessage::Error(Error::InvalidInput(
                                regex_string.to_owned(),
                            )))
                            .await;
                        return;
                    }
                };

                if regex.is_match(&string) {
                    let _ = channel
                        .send(ActionMessage::Element(Element::Text(string)))
                        .await;
                }
            }
            (Action::TextToHtml, Element::Text(string)) => {
                let _ = channel
                    .send(ActionMessage::Element(Element::Html(string)))
                    .await;
            }
            (Action::HtmlGetAttr(attr_name), Element::Html(html_string)) => {
                let html = Html::parse_fragment(&html_string);
                if let Some(attr_value) = html.fragment_root().and_then(|root| root.attr(attr_name))
                {
                    msgs_to_send.push(ActionMessage::Element(Element::Text(
                        attr_value.to_owned().into(),
                    )));
                }
            }
            (Action::TextToUrl, Element::Text(url_string)) => {
                let url = match Url::parse(&url_string) {
                    Ok(x) => x,
                    Err(_e) => {
                        let _ = channel
                            .send(ActionMessage::Error(Error::InvalidInput(
                                url_string.deref().into(),
                            )))
                            .await;
                        return;
                    }
                };

                let _ = channel
                    .send(ActionMessage::Element(Element::Url(url)))
                    .await;
            }
            (Action::UrlToText, Element::Url(url)) => {
                let _ = channel
                    .send(ActionMessage::Element(Element::Text(
                        url.to_string().into(),
                    )))
                    .await;
            }
            (Action::UrlFollowRedirect, Element::Url(url)) => {
                let redirected_url = if let Some(x) = url_cache.get(&url) {
                    x.deref().deref().clone()
                } else {
                    let mut header_map = HeaderMap::new();
                    header_map.append("User-Agent", HeaderValue::from_static("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"));
                    header_map.append("Dnt", HeaderValue::from_static("1"));
                    header_map.append("Sec-Fetch-Site", HeaderValue::from_static("none"));
                    header_map.append("Sec-Fetch-Dest", HeaderValue::from_static("document"));
                    header_map.append("Sec-Fetch-Mode", HeaderValue::from_static("navigate"));
                    header_map.append("Sec-Fetch-User", HeaderValue::from_static("?1"));
                    header_map.append("Accept", HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"));
                    header_map.append(
                        "Accept-Encoding",
                        HeaderValue::from_static("gzip, deflate, br"),
                    );
                    header_map.append("Accept-Language", HeaderValue::from_static("en"));

                    let client = match HttpClient::builder()
                        .default_headers(header_map)
                        .cookie_store(true)
                        .build()
                    {
                        Ok(x) => x,
                        Err(e) => {
                            eprintln!(
                                "/email/execute-script initialize HTTP client error: {:#?}",
                                e
                            );
                            let _ = channel
                                .send(ActionMessage::Error(Error::InternalError))
                                .await;
                            return;
                        }
                    };

                    let response = match client.get(url.clone()).send().await {
                        Ok(x) => x,
                        Err(e) => {
                            eprintln!("/email/execute-script HTTP error: {:#?}", e);
                            let _ = channel.send(ActionMessage::Done).await;
                            return;
                        }
                    };

                    url_cache.insert(url, response.url().clone());

                    response.url().clone()
                };

                let _ = channel
                    .send(ActionMessage::Element(Element::Url(redirected_url)))
                    .await;
            }
            (Action::UrlGetQuery(query_name), Element::Url(url)) => {
                if let Some(query_value) = url.query_pairs().find_map(|(key, value)| {
                    if &key == query_name {
                        Some(value)
                    } else {
                        None
                    }
                }) {
                    let _ = channel
                        .send(ActionMessage::Element(Element::Text(
                            query_value.to_string().into(),
                        )))
                        .await;
                }
            }
            (Action::EmailFilterRegex(email_attr, regex_string), Element::Email(email)) => {
                let regex = match Regex::new(regex_string) {
                    Ok(x) => x,
                    Err(_) => {
                        let _ = channel
                            .send(ActionMessage::Error(Error::InvalidInput(
                                regex_string.to_owned(),
                            )))
                            .await;
                        return;
                    }
                };

                let attr_value = email.get_attribute(*email_attr);

                if regex.is_match(attr_value) {
                    let _ = channel
                        .send(ActionMessage::Element(Element::Email(email)))
                        .await;
                }
            }
            (Action::UrlGetSegment(segment_index), Element::Url(url)) => {
                let mut segments = match url.path_segments() {
                    Some(x) => x,
                    None => {
                        eprintln!("/emails/execute-script URL path segments None");
                        let _ = channel.send(ActionMessage::Done).await;
                        return;
                    }
                };

                let segment_opt = if *segment_index < 0 {
                    segments.rev().nth((-*segment_index - 1) as usize)
                } else {
                    segments.nth(*segment_index as usize)
                };

                if let Some(segment) = segment_opt {
                    let _ = channel
                        .send(ActionMessage::Element(Element::Text(segment.into())))
                        .await;
                }
            }
            (Action::ArraySelectNth(target_index), el) => {
                if *target_index == element_index {
                    let _ = channel.send(ActionMessage::Element(el)).await;
                }
            }
            (Action::Or(actions1, actions2), el) => {
                let mut result = match exec_pipeline(
                    actions1,
                    Arc::clone(&config),
                    url_cache.clone(),
                    vec![el.clone()],
                )
                .await
                {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = channel.send(ActionMessage::Error(e)).await;
                        return;
                    }
                };

                if result.is_empty() {
                    result = match exec_pipeline(
                        actions2,
                        Arc::clone(&config),
                        url_cache.clone(),
                        vec![el],
                    )
                    .await
                    {
                        Ok(x) => x,
                        Err(e) => {
                            let _ = channel.send(ActionMessage::Error(e)).await;
                            return;
                        }
                    };
                }

                msgs_to_send.extend(result.into_iter().map(ActionMessage::Element));
            }
            (Action::EmailGetAttr(email_attr), Element::Email(email)) => {
                let attr = email.get_attribute(*email_attr);

                let _ = channel
                    .send(ActionMessage::Element(Element::Text(
                        attr.to_owned().into(),
                    )))
                    .await;
            }
            (Action::Pair(action1, action2), el) => {
                let elements1 = match exec_pipeline(
                    &*action1,
                    Arc::clone(&config),
                    url_cache.clone(),
                    vec![el.clone()],
                )
                .await
                {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = channel.send(ActionMessage::Error(e)).await;
                        return;
                    }
                };

                let elements2 = match exec_pipeline(
                    &*action2,
                    Arc::clone(&config),
                    url_cache.clone(),
                    vec![el],
                )
                .await
                {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = channel.send(ActionMessage::Error(e)).await;
                        return;
                    }
                };

                let _ = channel
                    .send(ActionMessage::Element(Element::Pair(elements1, elements2)))
                    .await;
            }
            (Action::Filter(actions), el) => {
                let elements = match exec_pipeline(
                    &*actions,
                    Arc::clone(&config),
                    url_cache,
                    vec![el.clone()],
                )
                .await
                {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = channel.send(ActionMessage::Error(e)).await;
                        return;
                    }
                };

                if !elements.is_empty() {
                    let _ = channel.send(ActionMessage::Element(el)).await;
                }
            }
            (Action::PairGetLeft, Element::Pair(elements1, _elements2)) => {
                msgs_to_send.extend(elements1.into_iter().map(ActionMessage::Element));
            }
            (Action::PairGetRight, Element::Pair(_elements1, elements2)) => {
                msgs_to_send.extend(elements2.into_iter().map(ActionMessage::Element));
            }
            (Action::PairZipTogether, Element::Pair(elements1, elements2)) => {
                msgs_to_send.extend(
                    elements1
                        .into_iter()
                        .zip(elements2.into_iter())
                        .map(|(a, b)| Element::Pair(vec![a], vec![b]))
                        .map(ActionMessage::Element),
                );
            }
            (Action::PairDistributeLeft, Element::Pair(elements1, elements2)) => {
                msgs_to_send.extend(elements2.into_iter().map(|el2| {
                    ActionMessage::Element(Element::Pair(elements1.clone(), vec![el2]))
                }));
            }
            (Action::PairRightLeft, Element::Pair(elements1, elements2)) => {
                let _ = channel
                    .send(ActionMessage::Element(Element::Pair(elements2, elements1)))
                    .await;
            }
            _ => {}
        }

        if let Some(error_msg) = error {
            let _ = channel.send(error_msg).await;
            return;
        }

        for msg in msgs_to_send {
            let _ = channel.send(msg).await;
        }

        let _ = channel.send(ActionMessage::Done).await;
    })
}

async fn exec_pipeline(
    actions: &[Action],
    config: ManagedConfig,
    url_cache: ManagedUrlCache,
    mut elements: Vec<Element>,
) -> Result<Vec<Element>, Error> {
    let mut expanded_actions = vec![];
    for action in actions {
        match action {
            Action::Macro(macro_name) => {
                match config.macros.iter().find(|mac| &mac.name == macro_name) {
                    Some(mac) => expanded_actions.extend(mac.actions.iter().cloned().map(Arc::new)),
                    None => return Err(Error::InvalidInput(macro_name.to_owned())),
                }
            }
            _ => expanded_actions.push(Arc::new(action.clone())),
        }
    }

    if expanded_actions.is_empty() {
        return Ok(elements);
    }

    for action in expanded_actions {
        if elements.is_empty() {
            return Ok(elements);
        }

        let (tx, mut rx) = mpsc::channel(16);
        let mut need_finish = elements.len();
        for (element_index, element) in elements.into_iter().enumerate() {
            tokio::spawn(exec_action(
                Arc::clone(&action),
                element_index,
                element,
                tx.clone(),
                Arc::clone(&config),
                url_cache.clone(),
            ));
        }

        let mut new_elements = vec![];
        loop {
            match rx.recv().await {
                Some(ActionMessage::Error(err)) => {
                    return Err(err);
                }
                Some(ActionMessage::Element(el)) => {
                    new_elements.push(el);
                }
                Some(ActionMessage::Done) => {
                    need_finish -= 1;
                    if need_finish == 0 {
                        elements = new_elements;
                        break;
                    }
                }
                None => {}
            }
        }
    }

    Ok(elements)
}

fn flatten_serde_pair(el: SerdeElement, v: &mut Vec<SerdeElement>) {
    match el {
        SerdeElement::Pair(left, right) => {
            if let Some(value) = left.into_iter().next() {
                flatten_serde_pair(value, v);
            }
            if let Some(value) = right.into_iter().next() {
                flatten_serde_pair(value, v);
            }
        }
        other => v.push(other),
    }
}

#[rocket::post("/emails/execute-script", format = "json", data = "<script>")]
pub async fn execute_script(
    user: AuthorizedUser<'_>,
    pool: &State<ManagedPool>,
    config: &State<ManagedConfig>,
    url_cache: &State<ManagedUrlCache>,
    script: Json<Script>,
    _ratelimit: Ratelimit,
) -> Result<
    FlexibleFormat<
        Vec<SerdeElement>,
        Vec<SerdeElement>,
        impl FnOnce(Vec<SerdeElement>) -> Vec<Vec<SerdeElement>>,
    >,
    Error,
> {
    let emails = match sqlx::query_as!(
        Email,
        r#"SELECT * FROM emails WHERE user = $1"#,
        user.username
    )
    .fetch_all(&**pool)
    .await
    {
        Ok(x) => x,
        Err(e) => {
            eprintln!("/emails/execute-script SQL error: {:#?}", e);
            return Err(Error::InternalError);
        }
    };

    let elements: Vec<_> = emails
        .into_iter()
        .map(Arc::new)
        .map(Element::Email)
        .collect();
    let pipelined = exec_pipeline(
        &script.actions,
        Arc::clone(&*config),
        (*url_cache).clone(),
        elements,
    )
    .await?;

    let mut formatted = FlexibleFormat::from_complex(
        pipelined
            .into_iter()
            .map(SerdeElement::from)
            .collect::<Vec<_>>(),
        |data| {
            data.into_iter()
                .map(|el| {
                    let mut v = vec![];
                    flatten_serde_pair(el, &mut v);
                    return v;
                })
                .collect()
        },
    );
    formatted.include_header(false);

    Ok(formatted)
}
