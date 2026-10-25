/** CSS.supports(gap) only tests grid gap on older WebKit, so measure flex layout. */
export function supportsFlexGap(document: Document): boolean {
  const probe = document.createElement("div");
  probe.style.cssText = "position:absolute;visibility:hidden;display:flex;flex-direction:column;row-gap:1px;padding:0;border:0;left:-9999px;top:0";
  for (let index = 0; index < 2; index++) {
    const child = document.createElement("div");
    child.style.cssText = "height:1px;flex:none;padding:0;margin:0;border:0";
    probe.appendChild(child);
  }
  try {
    (document.body || document.documentElement).appendChild(probe);
    return probe.scrollHeight === 3;
  } finally {
    probe.remove();
  }
}

/** Run before React/Tauri imports. Do not replace working native APIs. */
export function installBrowserCompatibility(host: Window & typeof globalThis): void {
  const supports = (property: string, value?: string): boolean => {
    try {
      return value === undefined ? host.CSS.supports(property) : host.CSS.supports(property, value);
    } catch {
      return false;
    }
  };
  const root = host.document.documentElement;
  root.classList.toggle("no-flex-gap", !supportsFlexGap(host.document));
  root.classList.toggle("no-focus-visible", !supports("selector(:focus-visible)"));
  root.classList.toggle("no-container-units", !supports("font-size", "1cqw"));
  root.classList.toggle("no-aspect-ratio", !supports("aspect-ratio", "1"));

  // Tauri's channel error handler uses queueMicrotask. Promise jobs retain FIFO
  // microtask ordering on older WebKit; callback exceptions must escape as errors,
  // rather than becoming unhandled promise rejections.
  if (typeof host.queueMicrotask !== "function") {
    const resolved = Promise.resolve();
    host.queueMicrotask = (callback: VoidFunction) => {
      if (typeof callback !== "function") throw new TypeError("queueMicrotask requires a function");
      void resolved.then(() => callback()).catch((error: unknown) => {
        host.setTimeout(() => { throw error; }, 0);
      });
    };
  }
}
