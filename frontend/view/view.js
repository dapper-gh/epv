(async function() {
    const id = new URL(location.href).searchParams.get("id");

    const appFrame = document.getElementById("app-frame");
    const appInfo = document.getElementById("app-info");

    const email = await fetch(`/api/emails/${id}`, {
        headers: {
            Authorization: localStorage.auth
        }
    }).then(r => r.json());

    const subjectP = document.createElement("p");
    subjectP.className = "bold";
    subjectP.innerText = email.subject;
    appInfo.appendChild(subjectP);

    const metaP = document.createElement("p");
    metaP.innerText = `From: ${email.from_addr}
To: ${email.to_addr}
Received: ${new Date(email.registered)}`;

    appInfo.appendChild(metaP);

    appFrame.src = `/api/emails/${id}/html?auth=${encodeURIComponent(localStorage.auth)}`;
})();