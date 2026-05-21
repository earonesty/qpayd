const DEFAULT_LIMIT = 25;
const TOKEN_STORAGE_PREFIX = "qpayd-admin-token";

export class QPaydAdminClient {
  constructor(options) {
    if (!options?.baseUrl) throw new Error("QPaydAdminClient requires baseUrl");
    if (!options?.storeId) throw new Error("QPaydAdminClient requires storeId");
    this.baseUrl = options.baseUrl.replace(/\/$/, "");
    this.storeId = options.storeId;
    this.token = options.token ?? "";
    this.fetch = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (!this.fetch) throw new Error("QPaydAdminClient requires fetch");
  }

  setToken(token) {
    this.token = token;
  }

  async listInvoices(options = {}) {
    const params = new URLSearchParams();
    params.set("limit", String(options.limit ?? DEFAULT_LIMIT));
    if (options.status) params.set("status", options.status);
    return this.#json(`/v1/stores/${encodeURIComponent(this.storeId)}/invoices?${params}`);
  }

  async getInvoice(invoiceId) {
    return this.#json(`/v1/stores/${encodeURIComponent(this.storeId)}/invoices/${encodeURIComponent(invoiceId)}`);
  }

  async getRefundSummary(invoiceId) {
    return this.#json(
      `/v1/stores/${encodeURIComponent(this.storeId)}/invoices/${encodeURIComponent(invoiceId)}/refund-summary`
    );
  }

  async createInvoiceRefund(invoiceId, refund) {
    return this.#json(
      `/v1/stores/${encodeURIComponent(this.storeId)}/invoices/${encodeURIComponent(invoiceId)}/refunds`,
      {
        method: "POST",
        body: JSON.stringify({
          amount_sats: refund.amountSats,
          destination: refund.destination || null,
          reason: refund.reason || null,
          metadata: refund.metadata ?? {}
        })
      }
    );
  }

  async finalizeRefund(refundId, options = {}) {
    return this.#json(`/v1/stores/${encodeURIComponent(this.storeId)}/refunds/${encodeURIComponent(refundId)}/finalize`, {
      method: "POST",
      body: JSON.stringify({ tx_id: options.txId || null })
    });
  }

  async cancelRefund(refundId) {
    return this.#json(`/v1/stores/${encodeURIComponent(this.storeId)}/refunds/${encodeURIComponent(refundId)}/cancel`, {
      method: "POST"
    });
  }

  async #json(path, init = {}) {
    if (!this.token) throw new Error("Admin token required");
    const response = await this.fetch(`${this.baseUrl}${path}`, {
      ...init,
      headers: {
        accept: "application/json",
        authorization: `Bearer ${this.token}`,
        ...(init.body ? { "content-type": "application/json" } : {}),
        ...(init.headers ?? {})
      }
    });
    if (!response.ok) {
      let message = `qpayd request failed with ${response.status}`;
      try {
        const body = await response.json();
        if (body?.error) message = body.error;
      } catch {
        // Keep the status-based message.
      }
      throw new Error(message);
    }
    return response.json();
  }
}

export function mountQPaydAdmin(target, options) {
  const root = typeof target === "string" ? document.querySelector(target) : target;
  if (!root) throw new Error("mountQPaydAdmin target not found");
  const client = options.client ?? new QPaydAdminClient(options);
  const storageKey = options.storageKey ?? `${TOKEN_STORAGE_PREFIX}:${client.baseUrl}:${client.storeId}`;
  const state = {
    client,
    storageKey,
    remember: false,
    invoices: [],
    selectedInvoice: null,
    refundSummary: null,
    status: "",
    error: "",
    notice: ""
  };

  const savedToken = options.rememberToken === false ? "" : localStorage.getItem(storageKey);
  if (savedToken) client.setToken(savedToken);

  installAdminStyles();
  render(root, state);
  if (client.token) refreshInvoices(root, state);
  return {
    client,
    refresh: () => refreshInvoices(root, state),
    destroy: () => {
      root.innerHTML = "";
    }
  };
}

function autoMountQPaydAdmin() {
  if (typeof document === "undefined") return;
  const script = document.querySelector("script[data-qpayd-admin]");
  if (!script) return;
  const target = script.dataset.target || "#qpayd-admin";
  mountQPaydAdmin(target, {
    baseUrl: script.dataset.baseUrl || window.location.origin,
    storeId: script.dataset.storeId
  });
}

autoMountQPaydAdmin();

function render(root, state) {
  root.className = "qpayd-admin";
  root.innerHTML = state.client.token ? appHtml(state) : loginHtml(state);
  bind(root, state);
}

function bind(root, state) {
  const login = root.querySelector("[data-qpayd-admin-login]");
  if (login) {
    login.addEventListener("submit", (event) => {
      event.preventDefault();
      const form = new FormData(login);
      const token = String(form.get("token") ?? "").trim();
      state.remember = form.get("remember") === "on";
      state.client.setToken(token);
      if (state.remember) localStorage.setItem(state.storageKey, token);
      state.error = "";
      render(root, state);
      refreshInvoices(root, state);
    });
    return;
  }

  root.querySelector("[data-qpayd-refresh]")?.addEventListener("click", () => refreshInvoices(root, state));
  root.querySelector("[data-qpayd-logout]")?.addEventListener("click", () => {
    localStorage.removeItem(state.storageKey);
    state.client.setToken("");
    state.invoices = [];
    state.selectedInvoice = null;
    state.refundSummary = null;
    render(root, state);
  });
  root.querySelector("[data-qpayd-status-filter]")?.addEventListener("change", (event) => {
    state.status = event.target.value;
    refreshInvoices(root, state);
  });
  root.querySelectorAll("[data-qpayd-invoice]").forEach((button) => {
    button.addEventListener("click", () => loadInvoice(root, state, button.dataset.qpaydInvoice));
  });
  root.querySelector("[data-qpayd-refund-form]")?.addEventListener("submit", (event) => {
    event.preventDefault();
    createRefund(root, state, event.currentTarget);
  });
  root.querySelector("[data-qpayd-scan]")?.addEventListener("click", () => scanQr(root, state));
  root.querySelectorAll("[data-qpayd-finalize]").forEach((button) => {
    button.addEventListener("click", () => finalizeRefund(root, state, button.dataset.qpaydFinalize));
  });
  root.querySelectorAll("[data-qpayd-cancel]").forEach((button) => {
    button.addEventListener("click", () => cancelRefund(root, state, button.dataset.qpaydCancel));
  });
}

async function refreshInvoices(root, state) {
  state.error = "";
  state.notice = "";
  render(root, state);
  try {
    state.invoices = await state.client.listInvoices({ status: state.status, limit: DEFAULT_LIMIT });
    if (!state.selectedInvoice && state.invoices[0]) {
      await loadInvoice(root, state, state.invoices[0].id, false);
      return;
    }
  } catch (error) {
    state.error = error.message;
  }
  render(root, state);
}

async function loadInvoice(root, state, invoiceId, rerender = true) {
  state.error = "";
  state.notice = "";
  if (rerender) render(root, state);
  try {
    state.selectedInvoice = await state.client.getInvoice(invoiceId);
    state.refundSummary = await state.client.getRefundSummary(invoiceId);
  } catch (error) {
    state.error = error.message;
  }
  render(root, state);
}

async function createRefund(root, state, form) {
  const amountSats = Number(form.amount_sats.value);
  const destination = form.destination.value.trim();
  const reason = form.reason.value.trim();
  state.error = "";
  state.notice = "";
  render(root, state);
  try {
    await state.client.createInvoiceRefund(state.selectedInvoice.id, {
      amountSats,
      destination,
      reason,
      metadata: { source: "qpayd-admin" }
    });
    state.notice = "Refund queued";
    await loadInvoice(root, state, state.selectedInvoice.id, false);
  } catch (error) {
    state.error = error.message;
    render(root, state);
  }
}

async function finalizeRefund(root, state, refundId) {
  const txId = window.prompt("Transaction id or payment proof");
  if (txId === null) return;
  state.error = "";
  try {
    await state.client.finalizeRefund(refundId, { txId: txId.trim() });
    state.notice = "Refund finalized";
    await loadInvoice(root, state, state.selectedInvoice.id, false);
  } catch (error) {
    state.error = error.message;
    render(root, state);
  }
}

async function cancelRefund(root, state, refundId) {
  state.error = "";
  try {
    await state.client.cancelRefund(refundId);
    state.notice = "Refund canceled";
    await loadInvoice(root, state, state.selectedInvoice.id, false);
  } catch (error) {
    state.error = error.message;
    render(root, state);
  }
}

async function scanQr(root, state) {
  if (!("BarcodeDetector" in window) || !navigator.mediaDevices?.getUserMedia) {
    state.error = "QR scanning is not available in this browser";
    render(root, state);
    return;
  }
  const overlay = document.createElement("div");
  overlay.className = "qpayd-scan";
  overlay.innerHTML = `
    <div class="qpayd-scan-box">
      <video autoplay playsinline></video>
      <button type="button">Cancel</button>
    </div>
  `;
  document.body.append(overlay);
  const video = overlay.querySelector("video");
  const stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" } });
  video.srcObject = stream;
  const detector = new BarcodeDetector({ formats: ["qr_code"] });
  let stopped = false;
  const stop = () => {
    stopped = true;
    stream.getTracks().forEach((track) => track.stop());
    overlay.remove();
  };
  overlay.querySelector("button").addEventListener("click", stop);
  const tick = async () => {
    if (stopped) return;
    const codes = await detector.detect(video).catch(() => []);
    if (codes[0]?.rawValue) {
      const input = root.querySelector("[name='destination']");
      if (input) input.value = codes[0].rawValue;
      stop();
      return;
    }
    window.requestAnimationFrame(tick);
  };
  tick();
}

function loginHtml(state) {
  return `
    <section class="qpayd-login" aria-label="qpayd admin login">
      <div>
        <h1>qpayd admin</h1>
        <p>${escapeHtml(state.client.storeId)} at ${escapeHtml(state.client.baseUrl)}</p>
      </div>
      <form data-qpayd-admin-login>
        <label>
          Admin token
          <input name="token" type="password" autocomplete="current-password" required autofocus>
        </label>
        <label class="qpayd-check">
          <input name="remember" type="checkbox">
          Remember this device
        </label>
        <button type="submit">Log in</button>
      </form>
      ${messageHtml(state)}
    </section>
  `;
}

function appHtml(state) {
  return `
    <div class="qpayd-shell">
      <aside class="qpayd-sidebar">
        <header>
          <div>
            <strong>qpayd</strong>
            <span>${escapeHtml(state.client.storeId)}</span>
          </div>
          <button type="button" data-qpayd-logout>Log out</button>
        </header>
        <div class="qpayd-toolbar">
          <select data-qpayd-status-filter aria-label="Invoice status">
            ${statusOptions(state.status)}
          </select>
          <button type="button" data-qpayd-refresh>Refresh</button>
        </div>
        <div class="qpayd-list">
          ${state.invoices.map((invoice) => invoiceRowHtml(invoice, state.selectedInvoice?.id)).join("")}
        </div>
      </aside>
      <main class="qpayd-main">
        ${messageHtml(state)}
        ${state.selectedInvoice ? invoiceDetailHtml(state) : `<section class="qpayd-empty">Select an invoice</section>`}
      </main>
    </div>
  `;
}

function invoiceDetailHtml(state) {
  const invoice = state.selectedInvoice;
  const summary = state.refundSummary;
  return `
    <section class="qpayd-detail">
      <header>
        <div>
          <h2>${escapeHtml(invoice.amount)} ${escapeHtml(invoice.currency)}</h2>
          <p>${escapeHtml(invoice.id)}</p>
        </div>
        <span data-status="${escapeHtml(invoice.status)}">${statusLabel(invoice.status)}</span>
      </header>
      <dl class="qpayd-metrics">
        <div><dt>Paid</dt><dd>${sats(invoice.paid_sats)}</dd></div>
        <div><dt>Due</dt><dd>${sats(invoice.remaining_sats)}</dd></div>
        <div><dt>Overpaid</dt><dd>${sats(invoice.overpaid_sats)}</dd></div>
        <div><dt>Refundable</dt><dd>${sats(summary?.refundable_sats ?? 0)}</dd></div>
      </dl>
      <section class="qpayd-refund">
        <h3>Create refund</h3>
        <form data-qpayd-refund-form>
          <label>
            Amount sats
            <input name="amount_sats" type="number" min="1" max="${summary?.refundable_sats ?? 0}" value="${summary?.refundable_sats ?? 0}" required>
          </label>
          <label>
            Destination
            <div class="qpayd-destination">
              <input name="destination" autocomplete="off" placeholder="Bitcoin address, URI, Lightning invoice, or note">
              <button type="button" data-qpayd-scan>Scan QR</button>
            </div>
          </label>
          <label>
            Reason
            <input name="reason" autocomplete="off" placeholder="overpayment, cancellation, support case">
          </label>
          <button type="submit" ${summary?.refundable_sats ? "" : "disabled"}>Queue refund</button>
        </form>
      </section>
      <section class="qpayd-refunds">
        <h3>Refunds</h3>
        ${(summary?.refunds ?? []).map(refundHtml).join("") || `<p>No refunds yet.</p>`}
      </section>
    </section>
  `;
}

function invoiceRowHtml(invoice, selectedId) {
  return `
    <button type="button" data-qpayd-invoice="${escapeHtml(invoice.id)}" aria-current="${invoice.id === selectedId}">
      <span>${escapeHtml(invoice.amount)} ${escapeHtml(invoice.currency)}</span>
      <strong>${sats(invoice.paid_sats)}</strong>
      <em>${statusLabel(invoice.status)}</em>
    </button>
  `;
}

function refundHtml(refund) {
  const pending = refund.status === "pending";
  return `
    <article class="qpayd-refund-row">
      <div>
        <strong>${sats(refund.amount_sats)}</strong>
        <span>${escapeHtml(refund.status)}</span>
        <p>${escapeHtml(refund.destination || "No destination recorded")}</p>
      </div>
      <div class="qpayd-refund-actions">
        ${pending ? `<button type="button" data-qpayd-finalize="${escapeHtml(refund.id)}">Finalize</button>` : ""}
        ${pending ? `<button type="button" data-qpayd-cancel="${escapeHtml(refund.id)}">Cancel</button>` : ""}
      </div>
    </article>
  `;
}

function statusOptions(selected) {
  const options = [
    ["", "All"],
    ["new", "New"],
    ["payment_detected", "Detected"],
    ["partially_paid", "Partial"],
    ["settled", "Settled"],
    ["expired", "Expired"],
    ["paid_late", "Paid late"]
  ];
  return options
    .map(([value, label]) => `<option value="${value}" ${value === selected ? "selected" : ""}>${label}</option>`)
    .join("");
}

function statusLabel(status) {
  return {
    new: "New",
    payment_detected: "Detected",
    partially_paid: "Partial",
    settled: "Settled",
    expired: "Expired",
    paid_late: "Paid late",
    invalid: "Invalid"
  }[status] ?? status;
}

function messageHtml(state) {
  return `
    ${state.error ? `<p class="qpayd-message qpayd-error">${escapeHtml(state.error)}</p>` : ""}
    ${state.notice ? `<p class="qpayd-message qpayd-notice">${escapeHtml(state.notice)}</p>` : ""}
  `;
}

function sats(value) {
  return `${Number(value ?? 0).toLocaleString()} sats`;
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    "\"": "&quot;",
    "'": "&#39;"
  })[char]);
}

function installAdminStyles() {
  if (document.getElementById("qpayd-admin-styles")) return;
  const style = document.createElement("style");
  style.id = "qpayd-admin-styles";
  style.textContent = `
    .qpayd-admin { min-height: 100vh; background: #f5f6f2; color: #17201b; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    .qpayd-admin * { box-sizing: border-box; }
    .qpayd-login { width: min(420px, calc(100vw - 32px)); margin: 12vh auto 0; padding: 24px; border: 1px solid #d8ddd2; border-radius: 8px; background: #ffffff; box-shadow: 0 18px 60px rgba(32, 38, 34, .12); }
    .qpayd-login h1 { margin: 0; font-size: 28px; line-height: 1.1; }
    .qpayd-login p { margin: 8px 0 24px; color: #5d6b61; overflow-wrap: anywhere; }
    .qpayd-login form, .qpayd-refund form { display: grid; gap: 14px; }
    .qpayd-admin label { display: grid; gap: 6px; font-size: 13px; font-weight: 700; color: #34443a; }
    .qpayd-admin input, .qpayd-admin select { min-height: 40px; width: 100%; border: 1px solid #cbd3c6; border-radius: 8px; background: #fff; color: #17201b; padding: 8px 10px; font: inherit; }
    .qpayd-admin button { min-height: 40px; border: 1px solid #b9c4b4; border-radius: 8px; background: #ffffff; color: #17201b; padding: 8px 12px; font: inherit; font-weight: 800; cursor: pointer; }
    .qpayd-admin button:hover { border-color: #6f8f72; }
    .qpayd-admin button:disabled { color: #8d968f; cursor: not-allowed; }
    .qpayd-login button[type="submit"], .qpayd-refund button[type="submit"] { background: #2f6f4e; border-color: #2f6f4e; color: #fff; }
    .qpayd-check { display: flex !important; grid-template-columns: none; align-items: center; gap: 8px; font-weight: 600; }
    .qpayd-check input { width: 16px; min-height: 16px; }
    .qpayd-shell { display: grid; grid-template-columns: minmax(280px, 360px) 1fr; min-height: 100vh; }
    .qpayd-sidebar { border-right: 1px solid #d8ddd2; background: #ffffff; min-width: 0; }
    .qpayd-sidebar header, .qpayd-detail header { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; padding: 16px; border-bottom: 1px solid #d8ddd2; }
    .qpayd-sidebar strong { display: block; font-size: 20px; }
    .qpayd-sidebar span, .qpayd-detail p { color: #5d6b61; overflow-wrap: anywhere; }
    .qpayd-toolbar { display: grid; grid-template-columns: 1fr auto; gap: 8px; padding: 12px; border-bottom: 1px solid #d8ddd2; }
    .qpayd-list { display: grid; max-height: calc(100vh - 122px); overflow: auto; }
    .qpayd-list button { display: grid; grid-template-columns: 1fr auto; gap: 4px 10px; min-height: 72px; padding: 12px; border: 0; border-bottom: 1px solid #ecf0e8; border-radius: 0; text-align: left; }
    .qpayd-list button[aria-current="true"] { background: #e8f2e9; }
    .qpayd-list em { color: #5d6b61; font-style: normal; font-size: 13px; }
    .qpayd-main { padding: 18px; min-width: 0; }
    .qpayd-detail { display: grid; gap: 16px; max-width: 980px; }
    .qpayd-detail header { border: 1px solid #d8ddd2; border-radius: 8px; background: #fff; }
    .qpayd-detail h2 { margin: 0 0 6px; font-size: 24px; }
    .qpayd-detail header span { border: 1px solid #cbd3c6; border-radius: 8px; padding: 6px 10px; background: #f8faf6; font-weight: 800; }
    .qpayd-metrics { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; margin: 0; }
    .qpayd-metrics div, .qpayd-refund, .qpayd-refunds, .qpayd-empty { border: 1px solid #d8ddd2; border-radius: 8px; background: #fff; padding: 16px; }
    .qpayd-metrics dt { color: #5d6b61; font-size: 13px; }
    .qpayd-metrics dd { margin: 4px 0 0; font-size: 18px; font-weight: 900; }
    .qpayd-refund h3, .qpayd-refunds h3 { margin: 0 0 14px; font-size: 18px; }
    .qpayd-destination { display: grid; grid-template-columns: 1fr auto; gap: 8px; }
    .qpayd-refund-row { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; padding: 12px 0; border-top: 1px solid #ecf0e8; }
    .qpayd-refund-row p { margin: 4px 0 0; color: #5d6b61; overflow-wrap: anywhere; }
    .qpayd-refund-actions { display: flex; gap: 8px; }
    .qpayd-message { margin: 0 0 12px; padding: 10px 12px; border-radius: 8px; font-weight: 800; }
    .qpayd-error { background: #ffe9e4; color: #8d2718; }
    .qpayd-notice { background: #e2f4e6; color: #225d39; }
    .qpayd-scan { position: fixed; inset: 0; z-index: 10000; display: grid; place-items: center; background: rgba(20, 28, 23, .72); padding: 16px; }
    .qpayd-scan-box { display: grid; gap: 12px; width: min(520px, 100%); padding: 12px; border-radius: 8px; background: #fff; }
    .qpayd-scan video { width: 100%; aspect-ratio: 1; object-fit: cover; border-radius: 8px; background: #111; }
    @media (max-width: 760px) {
      .qpayd-shell { grid-template-columns: 1fr; }
      .qpayd-sidebar { border-right: 0; border-bottom: 1px solid #d8ddd2; }
      .qpayd-list { max-height: 320px; }
      .qpayd-metrics { grid-template-columns: repeat(2, minmax(0, 1fr)); }
      .qpayd-destination { grid-template-columns: 1fr; }
    }
  `;
  document.head.append(style);
}
