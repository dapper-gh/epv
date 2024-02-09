(async function() {
    function renderElement(element) {
        const elementP = document.createElement("p");
        elementP.innerText = element.type;
        elementP.className = "bold";

        if (Array.isArray(element.value)) {
            const blockquotes = [];

            for (const [index, sublist] of element.value.entries()) {
                const expandBtn = document.createElement("input");
                expandBtn.type = "button";
                expandBtn.className = index === 0 ? "expand element-value" : "expand";
                const designator = element.value.length === 2 ? (index === 1 ? "Right" : "Left") : (index + 1);
                expandBtn.value = `Expand ${designator}`;

                const listBlockquote = document.createElement("blockquote");
                listBlockquote.style.display = "none";
                for (const innerElement of sublist) {
                    listBlockquote.appendChild(renderElement(innerElement));
                }

                let expanded = false;
                expandBtn.addEventListener("click", function() {
                    expanded = !expanded;
                    expandBtn.value = `${expanded ? "Hide" : "Expand"} ${designator}`;

                    listBlockquote.style.display = expanded ? "block" : "none";
                });

                elementP.appendChild(expandBtn);
                blockquotes.push(listBlockquote);
            }

            for (const blockquote of blockquotes) {
                elementP.appendChild(blockquote);
            }
        } else {
            const valueBlockquote = document.createElement("span");
            valueBlockquote.innerText = element.value;
            valueBlockquote.className = "normal-weight element-value";

            elementP.appendChild(valueBlockquote);
        }

        return elementP;
    }

    const blockActions = {
        Or: 2,
        Pair: 2,
        Filter: 1
    };

    const appScript = document.getElementById("app-script");
    const appExecute = document.getElementById("app-execute");
    const appOutput = document.getElementById("app-output");

    appExecute.addEventListener("click", async function() {
        const lines = appScript.value.split("\n");

        let payload = {actions: []};
        let lastProcessedAction = null;
        for (let i = 0; i < lines.length; i++) {
            (function processLine(context) {
                let line = lines[i]?.trim?.();

                if (!line) return {done: true};
                if (line === "{") return {done: false};
                if (line === "}") return {done: true};

                if (line.endsWith("{")) line = line.slice(0, -1).trim();

                const [actionName, ...actionFirstArgumentParts] = line.split(" ");
                const actionFirstArgumentString = actionFirstArgumentParts.join(" ");
                const actionFirstArgument = /^-\d+(?:\.\d+)?$/.test(actionFirstArgumentString) ? parseFloat(actionFirstArgumentString) : actionFirstArgumentString;

                if (actionName === "Additional") {
                    if (!lastProcessedAction.arguments) lastProcessedAction.arguments = actionFirstArgument;
                    else {
                        if (!Array.isArray(lastProcessedAction.arguments)) lastProcessedAction.arguments = [lastProcessedAction.arguments];
                        lastProcessedAction.arguments.push(actionFirstArgument);
                    }
                } else {
                    if (actionFirstArgumentParts.length) {
                        context.push({name: actionName, arguments: actionFirstArgument});
                    } else {
                        context.push({name: actionName});
                    }

                    lastProcessedAction = context[context.length - 1];

                    if (blockActions.hasOwnProperty(actionName)) {
                        const action = lastProcessedAction;
                        action.arguments = [];

                        for (let j = 0; j < blockActions[actionName]; j++) {
                            let arr;
                            if (blockActions[actionName] === 1) {
                                arr = action.arguments;
                            } else {
                                arr = [];
                                action.arguments.push(arr);
                            }
                            let done = false;
                            do {
                                i++;
                                done = processLine(arr).done;
                            } while (!done);
                        }
                    }
                }

                return {done: false};
            })(payload.actions);
        }

        const response = await fetch("/api/emails/execute-script", {
            method: "POST",
            headers: {
                Authorization: localStorage.auth,
                "Content-Type": "application/json"
            },
            body: JSON.stringify(payload)
        }).then(r => r.json());

        appOutput.innerHTML = "";
        for (const element of response) {
            appOutput.appendChild(renderElement(element));
        }
    });
})();