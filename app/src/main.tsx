import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import App from "./App";
import { reportUiError } from "./ipc";

// errors inside the window are otherwise invisible; send them to deej's log
window.addEventListener("error", (e) => reportUiError(`${e.message} at ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => reportUiError(`unhandled: ${String(e.reason)}`));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
