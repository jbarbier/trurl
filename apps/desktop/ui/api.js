// The only file in the UI that knows where the core lives.
//
//   Tauri desktop app  -> window.__TAURI__ (invoke + events)
//   dev bridge         -> HTTP POST /api/<command> + Server-Sent Events on /events
//   ?mock              -> mock.js, a scripted fake core for design work and tests
//
// Contract: contracts/events.md. Every command returns a Promise and rejects
// with a UserError object { code, title, hint, detail? }.

(function () {
  "use strict";

  const listeners = new Set();
  const emit = (ev) => listeners.forEach((fn) => fn(ev));

  function tauriApi() {
    const T = window.__TAURI__;
    T.event.listen("trurl", (e) => emit(e.payload));
    return {
      kind: "tauri",
      // One Tauri command, `call`, forwards to trurl_core::api::dispatch (same as the bridge).
      call: (cmd, args) => T.core.invoke("call", { cmd, args: args || {} }).catch((e) => Promise.reject(normalizeError(e))),
      pickModelFile: async () => {
        const path = await T.dialog.open({
          multiple: false,
          directory: false,
          filters: [{ name: "GGUF model", extensions: ["gguf", "GGUF"] }],
        });
        return typeof path === "string" ? path : null;
      },
    };
  }

  function bridgeApi() {
    let es = null;
    const connect = () => {
      es = new EventSource("/events");
      es.onmessage = (m) => {
        try {
          emit(JSON.parse(m.data));
        } catch (_) {}
      };
      es.onerror = () => {
        es.close();
        setTimeout(connect, 1000);
      };
    };
    connect();
    return {
      kind: "bridge",
      call: async (cmd, args) => {
        const r = await fetch("/api/" + cmd, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(args || {}),
        });
        const body = await r.json().catch(() => null);
        if (!r.ok) throw normalizeError(body);
        return body;
      },
      // The browser cannot hand us a real path; the bridge asks for one.
      pickModelFile: async () => window.prompt("Path of a .gguf file on the computer running the bridge:") || null,
    };
  }

  function normalizeError(e) {
    if (e && typeof e === "object" && e.title) return e;
    return { code: "internal", title: "Something went wrong inside Trurl.", hint: "Try again.", detail: String(e && e.message ? e.message : e) };
  }

  let impl;
  const params = new URLSearchParams(location.search);
  if (window.__TAURI__) impl = tauriApi();
  else if (params.has("mock") && window.TrurlMock) impl = window.TrurlMock.create(emit);
  else impl = bridgeApi();

  window.trurl = {
    kind: impl.kind,
    on: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    call: impl.call,
    pickModelFile: impl.pickModelFile,
  };
})();
