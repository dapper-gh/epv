(async function() {
    if (localStorage.auth) {
        const app = document.getElementById("app");
        app.style.display = "block";

        const emailList = document.getElementById("app-email-list");

        const response = await fetch("/api/emails/list", {
            headers: {
                Authorization: localStorage.auth
            }
        }).then(r => r.json());

        let atLeastOne = false;
        for (const email of response) {
            if (atLeastOne) {
                const emailHr = document.createElement("hr");
                emailList.append(emailHr);
            }

            const emailDiv = document.createElement("div");

            const subjectP = document.createElement("p");
            subjectP.className = "bold";
            subjectP.innerText = email.subject;
            emailDiv.appendChild(subjectP);

            const metaP = document.createElement("p");
            metaP.innerText = `From: ${email.from_addr}
To: ${email.to_addr}
Received: ${new Date(email.registered)}`;

            const openP = document.createElement("p");

            const openA = document.createElement("a");
            openA.href = `/view/?id=${email.id}`;
            openA.innerText = "Open!";
            openP.appendChild(openA);

            metaP.appendChild(openP);

            emailDiv.appendChild(metaP);

            emailList.appendChild(emailDiv);
            atLeastOne = true;
        }
    } else {
        const login = document.getElementById("login");
        login.style.display = "block";

        const loginUsername = document.getElementById("login-username");
        const loginPassword = document.getElementById("login-password");
        const loginSubmit = document.getElementById("login-submit");

        loginSubmit.addEventListener("click", async function() {
            const auth = `${loginUsername.value}:${loginPassword.value}`;

            const response = await fetch("/api/auth/verify", {
                headers: {
                    Authorization: auth
                }
            }).then(r => r.json());
            if (response.verified) {
                localStorage.auth = auth;
                location.reload();
            }
        });
    }
})();