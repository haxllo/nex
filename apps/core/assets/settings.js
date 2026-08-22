function send() {
    const v =document.getElementById("val").value;
    window.chrome.webview.postMessage(JSON.stringify({ t: "setSetting", v}));
    document.getElementById("status").textContent = "sent: " + v;
}
window.chrome.webview.addEventListener("message", (e) => {
    document.getElementById("status").textContent = "host says: " + e.data.v;
});