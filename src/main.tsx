import "./compat/bootstrap";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTextSizes, loadTextSizes } from "./preferences/textSizes";

applyTextSizes(loadTextSizes());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
