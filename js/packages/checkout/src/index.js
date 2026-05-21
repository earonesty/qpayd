const DEFAULT_POLL_INTERVAL_MS = 2500;
const TERMINAL_STATUSES = new Set(["settled", "expired", "paid_late", "invalid"]);
const ABBREVIATE_AT = 48;
const ABBREVIATE_HEAD = 18;
const ABBREVIATE_TAIL = 14;
const PAYMENT_METHOD_STORAGE_KEY = "qpayd-checkout-payment-method";

export class QPaydClient {
  constructor(options) {
    if (!options?.baseUrl) {
      throw new Error("QPaydClient requires baseUrl");
    }
    this.baseUrl = options.baseUrl.replace(/\/$/, "");
    this.fetch = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (!this.fetch) {
      throw new Error("QPaydClient requires fetch");
    }
  }

  async createPaymentLinkInvoice(storeId, paymentLinkId, options = {}) {
    const headers = {};
    if (options.idempotencyKey) headers["Idempotency-Key"] = options.idempotencyKey;
    return this.#json(
      `/v1/public/stores/${encodeURIComponent(storeId)}/payment-links/${encodeURIComponent(paymentLinkId)}/invoices`,
      { method: "POST", headers }
    );
  }

  async getInvoice(storeId, invoiceId) {
    return this.#json(
      `/v1/public/stores/${encodeURIComponent(storeId)}/invoices/${encodeURIComponent(invoiceId)}`
    );
  }

  resolveUrl(path) {
    if (!path) return "";
    if (/^https?:\/\//i.test(path)) return path;
    return `${this.baseUrl}${path.startsWith("/") ? "" : "/"}${path}`;
  }

  async #json(path, init) {
    const response = await this.fetch(this.resolveUrl(path), {
      ...init,
      headers: {
        accept: "application/json",
        ...(init?.headers ?? {})
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

export async function openPaymentLink(options) {
  const client = options.client ?? new QPaydClient({ baseUrl: options.baseUrl });
  let invoice;
  try {
    invoice = await client.createPaymentLinkInvoice(options.storeId, options.paymentLinkId, {
      idempotencyKey: options.idempotencyKey
    });
  } catch (error) {
    if (options.showErrors !== false && typeof document !== "undefined") {
      return openCheckoutErrorModal(error);
    }
    throw error;
  }
  return openInvoiceModal({
    ...options,
    client,
    invoice
  });
}

export function openInvoiceModal(options) {
  if (!options?.client) throw new Error("openInvoiceModal requires client");
  if (!options?.invoice) throw new Error("openInvoiceModal requires invoice");

  installStyles();

  const state = {
    client: options.client,
    invoice: options.invoice,
    pollIntervalMs: options.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS,
    onSettled: options.onSettled,
    onExpired: options.onExpired,
    selectedMethod: preferredPaymentMethod(options.invoice),
    timer: 0,
    closed: false
  };

  const root = document.createElement("div");
  root.className = "qpayd-modal-root";
  root.innerHTML = modalHtml(state.invoice);
  document.body.append(root);

  const close = () => {
    state.closed = true;
    if (state.timer) window.clearTimeout(state.timer);
    root.remove();
  };

  root.querySelector("[data-qpayd-close]").addEventListener("click", close);
  root.addEventListener("click", (event) => {
    if (event.target === root.querySelector(".qpayd-backdrop")) close();
  });
  root.querySelectorAll("[data-qpayd-copy]").forEach((button) => {
    button.addEventListener("click", async () => {
      const target = root.querySelector(button.getAttribute("data-qpayd-copy"));
      const value = target?.dataset.qpaydFullValue ?? target?.textContent?.trim();
      if (!value) return;
      await navigator.clipboard.writeText(value);
      button.textContent = "Copied";
      window.setTimeout(() => {
        button.textContent = "Copy";
      }, 1200);
    });
  });
  root.querySelectorAll("[data-qpayd-value]").forEach((button) => {
    button.addEventListener("click", () => {
      toggleValue(button);
    });
  });
  root.querySelectorAll("[data-qpayd-method]").forEach((button) => {
    button.addEventListener("click", () => {
      const method = button.getAttribute("data-qpayd-method");
      if (setMethod(root, method)) {
        state.selectedMethod = method;
        savePreferredPaymentMethod(method);
      }
    });
  });

  renderInvoice(root, state.client, state.invoice);
  if (!setMethod(root, state.selectedMethod)) {
    state.selectedMethod = defaultPaymentMethod(state.invoice);
    setMethod(root, state.selectedMethod);
  }
  poll(root, state);

  return {
    close,
    invoice: state.invoice,
    element: root
  };
}

function openCheckoutErrorModal(error) {
  installStyles();

  const root = document.createElement("div");
  root.className = "qpayd-modal-root";
  root.innerHTML = `
    <div class="qpayd-backdrop"></div>
    <section class="qpayd-modal" role="dialog" aria-modal="true" aria-label="Payment unavailable">
      <header class="qpayd-head">
        <div>
          <strong>Payment unavailable</strong>
          <span>Please try again shortly.</span>
        </div>
        <button type="button" data-qpayd-close aria-label="Close">x</button>
      </header>
      <p class="qpayd-error qpayd-error-panel">${escapeHtml(error.message)}</p>
    </section>
  `;
  document.body.append(root);

  const close = () => root.remove();
  root.querySelector("[data-qpayd-close]").addEventListener("click", close);
  root.addEventListener("click", (event) => {
    if (event.target === root.querySelector(".qpayd-backdrop")) close();
  });

  return {
    close,
    error,
    element: root
  };
}

async function poll(root, state) {
  if (state.closed || TERMINAL_STATUSES.has(state.invoice.status)) {
    handleTerminal(state);
    return;
  }
  state.timer = window.setTimeout(async () => {
    try {
      state.invoice = await state.client.getInvoice(state.invoice.store_id, state.invoice.id);
      renderInvoice(root, state.client, state.invoice);
      if (!setMethod(root, state.selectedMethod)) {
        state.selectedMethod = defaultPaymentMethod(state.invoice);
        setMethod(root, state.selectedMethod);
      }
    } catch (error) {
      root.querySelector("[data-qpayd-error]").textContent = error.message;
    }
    poll(root, state);
  }, state.pollIntervalMs);
}

function handleTerminal(state) {
  if (state.invoice.status === "settled") state.onSettled?.(state.invoice);
  if (state.invoice.status === "expired") state.onExpired?.(state.invoice);
}

function renderInvoice(root, client, invoice) {
  root.querySelector("[data-qpayd-status]").textContent = statusLabel(invoice.status);
  root.querySelector("[data-qpayd-status]").dataset.status = invoice.status;
  root.querySelector("[data-qpayd-sats]").textContent = satsLabel(invoice);
  root.querySelector("[data-qpayd-fiat]").textContent = `${invoice.amount} ${invoice.currency}`;
  root.querySelector("[data-qpayd-expiry]").textContent = formatExpiry(invoice.expires_at);

  renderMethod(root, client, "bitcoin", invoice.bitcoin, "address");
  renderMethod(root, client, "lightning", invoice.lightning, "bolt11");
}

function renderMethod(root, client, method, payment, valueKey) {
  const panel = root.querySelector(`[data-qpayd-panel="${method}"]`);
  const tab = root.querySelector(`[data-qpayd-method="${method}"]`);
  if (!payment) {
    panel.hidden = true;
    tab.hidden = true;
    return;
  }
  tab.hidden = false;
  panel.hidden = false;
  panel.querySelector("[data-qpayd-qr]").src = client.resolveUrl(payment.qr_svg_url);
  setPaymentValue(panel.querySelector("[data-qpayd-value]"), payment[valueKey]);
  setWalletHref(panel.querySelector("[data-qpayd-uri]"), payment.uri);
}

function setMethod(root, method) {
  const selectedTab = root.querySelector(`[data-qpayd-method="${method}"]`);
  const selectedPanel = root.querySelector(`[data-qpayd-panel="${method}"]`);
  if (!selectedTab || !selectedPanel || selectedTab.hidden || selectedPanel.hidden) return false;

  root.querySelectorAll("[data-qpayd-method]").forEach((button) => {
    button.setAttribute("aria-selected", String(button.getAttribute("data-qpayd-method") === method));
  });
  root.querySelectorAll("[data-qpayd-panel]").forEach((panel) => {
    panel.hidden = panel.getAttribute("data-qpayd-panel") !== method;
  });
  return true;
}

function preferredPaymentMethod(invoice) {
  const saved = loadPreferredPaymentMethod();
  return paymentMethodAvailable(invoice, saved) ? saved : defaultPaymentMethod(invoice);
}

function defaultPaymentMethod(invoice) {
  return invoice.lightning ? "lightning" : "bitcoin";
}

function paymentMethodAvailable(invoice, method) {
  return (method === "lightning" && Boolean(invoice.lightning)) || (method === "bitcoin" && Boolean(invoice.bitcoin));
}

function loadPreferredPaymentMethod() {
  try {
    const method = globalThis.localStorage?.getItem(PAYMENT_METHOD_STORAGE_KEY);
    return method === "bitcoin" || method === "lightning" ? method : "";
  } catch {
    return "";
  }
}

function savePreferredPaymentMethod(method) {
  try {
    globalThis.localStorage?.setItem(PAYMENT_METHOD_STORAGE_KEY, method);
  } catch {
    // Preference storage is optional.
  }
}

function modalHtml(invoice) {
  return `
    <div class="qpayd-backdrop"></div>
    <section class="qpayd-modal" role="dialog" aria-modal="true" aria-label="Bitcoin payment">
      <header class="qpayd-head">
        <div>
          <strong>Pay with Bitcoin</strong>
          <span data-qpayd-fiat>${escapeHtml(invoice.amount)} ${escapeHtml(invoice.currency)}</span>
        </div>
        <button type="button" data-qpayd-close aria-label="Close">x</button>
      </header>
      <div class="qpayd-summary">
        <span data-qpayd-status data-status="${escapeHtml(invoice.status)}">${escapeHtml(statusLabel(invoice.status))}</span>
        <span data-qpayd-sats>${invoice.btc_amount_sats.toLocaleString()} sats</span>
        <span data-qpayd-expiry>${formatExpiry(invoice.expires_at)}</span>
      </div>
      <nav class="qpayd-tabs" aria-label="Payment method">
        <button type="button" data-qpayd-method="bitcoin">Bitcoin</button>
        <button type="button" data-qpayd-method="lightning">Lightning</button>
      </nav>
      <div class="qpayd-panel" data-qpayd-panel="bitcoin">
        <img data-qpayd-qr alt="Bitcoin payment QR code">
        <button type="button" class="qpayd-value" data-qpayd-value aria-label="Show full Bitcoin payment value"></button>
        <div class="qpayd-actions">
          <button type="button" data-qpayd-copy='[data-qpayd-panel="bitcoin"] [data-qpayd-value]'>Copy</button>
          <a data-qpayd-uri>Open wallet</a>
        </div>
      </div>
      <div class="qpayd-panel" data-qpayd-panel="lightning">
        <img data-qpayd-qr alt="Lightning payment QR code">
        <button type="button" class="qpayd-value" data-qpayd-value aria-label="Show full Lightning invoice"></button>
        <div class="qpayd-actions">
          <button type="button" data-qpayd-copy='[data-qpayd-panel="lightning"] [data-qpayd-value]'>Copy</button>
          <a data-qpayd-uri>Open wallet</a>
        </div>
      </div>
      <p class="qpayd-note">This window updates automatically after payment. Fulfillment is confirmed by signed webhook.</p>
      <p class="qpayd-error" data-qpayd-error></p>
    </section>
  `;
}

function statusLabel(status) {
  return {
    new: "Awaiting payment",
    payment_detected: "Payment detected",
    partially_paid: "Partially paid",
    settled: "Paid",
    expired: "Expired",
    paid_late: "Paid late",
    invalid: "Invalid"
  }[status] ?? status;
}

function satsLabel(invoice) {
  if (invoice.overpaid_sats > 0) {
    return `${invoice.paid_sats.toLocaleString()} sats paid`;
  }
  if (invoice.paid_sats > 0 && invoice.remaining_sats > 0) {
    return `${invoice.remaining_sats.toLocaleString()} sats due`;
  }
  return `${invoice.btc_amount_sats.toLocaleString()} sats`;
}

function formatExpiry(value) {
  const expires = new Date(value);
  if (!Number.isFinite(expires.getTime())) return "";
  return `Expires ${expires.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`;
}

function setPaymentValue(element, value) {
  element.dataset.qpaydFullValue = value;
  element.dataset.qpaydExpanded = "false";
  element.textContent = abbreviate(value);
  element.title = value.length > ABBREVIATE_AT ? "Click to show full value" : "";
}

function setWalletHref(element, uri) {
  const value = String(uri ?? "");
  if (/^(bitcoin|lightning):/i.test(value)) {
    element.href = value;
    element.removeAttribute("aria-disabled");
  } else {
    element.removeAttribute("href");
    element.setAttribute("aria-disabled", "true");
  }
}

function toggleValue(element) {
  const value = element.dataset.qpaydFullValue ?? "";
  if (!value || value.length <= ABBREVIATE_AT) return;
  const expanded = element.dataset.qpaydExpanded === "true";
  element.dataset.qpaydExpanded = String(!expanded);
  element.textContent = expanded ? abbreviate(value) : value;
  element.title = expanded ? "Click to show full value" : "Click to abbreviate";
}

function abbreviate(value) {
  if (value.length <= ABBREVIATE_AT) return value;
  return `${value.slice(0, ABBREVIATE_HEAD)}...${value.slice(-ABBREVIATE_TAIL)}`;
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

function installStyles() {
  if (document.getElementById("qpayd-modal-styles")) return;
  const style = document.createElement("style");
  style.id = "qpayd-modal-styles";
  style.textContent = `
    .qpayd-modal-root { position: fixed; inset: 0; z-index: 9999; display: grid; place-items: center; padding: 16px; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #edf4f1; }
    .qpayd-modal-root * { box-sizing: border-box; }
    .qpayd-backdrop { position: absolute; inset: 0; background: rgba(3, 7, 10, .72); backdrop-filter: blur(8px); }
    .qpayd-modal { position: relative; width: min(420px, 100%); max-height: min(720px, calc(100vh - 32px)); overflow: auto; border: 1px solid #27343f; border-radius: 8px; background: #111820; box-shadow: 0 24px 80px rgba(0,0,0,.38); }
    .qpayd-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; padding: 18px; border-bottom: 1px solid #27343f; }
    .qpayd-head strong { display: block; font-size: 20px; line-height: 1.15; }
    .qpayd-head span { display: block; margin-top: 4px; color: #9fb0aa; }
    .qpayd-head button { width: 32px; height: 32px; border: 1px solid #27343f; border-radius: 8px; background: #17212b; color: #edf4f1; cursor: pointer; }
    .qpayd-summary { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; padding: 14px 18px; border-bottom: 1px solid #27343f; color: #9fb0aa; font-size: 14px; }
    .qpayd-summary [data-qpayd-status] { color: #ffd166; font-weight: 800; }
    .qpayd-summary [data-status="settled"] { color: #6ee7a8; }
    .qpayd-tabs { display: flex; gap: 8px; padding: 14px 18px 0; }
    .qpayd-tabs button { flex: 1; min-height: 38px; border: 1px solid #27343f; border-radius: 8px; background: #17212b; color: #edf4f1; cursor: pointer; }
    .qpayd-tabs button[aria-selected="true"] { background: #6ee7a8; border-color: #6ee7a8; color: #08100c; font-weight: 800; }
    .qpayd-panel { padding: 18px; }
    .qpayd-panel[hidden], .qpayd-tabs button[hidden] { display: none; }
    .qpayd-panel img { display: block; width: min(260px, 100%); aspect-ratio: 1; margin: 0 auto 16px; border-radius: 8px; background: #fff; }
    .qpayd-value { display: block; width: 100%; padding: 12px; min-height: 44px; border: 1px solid #27343f; border-radius: 8px; background: #0b1116; color: #d6e3df; overflow-wrap: anywhere; text-align: left; cursor: pointer; font: 12px/1.55 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    .qpayd-value[data-qpayd-expanded="true"] { max-height: 112px; overflow: auto; }
    .qpayd-actions { display: flex; gap: 10px; margin-top: 12px; }
    .qpayd-actions button, .qpayd-actions a { flex: 1; display: inline-flex; align-items: center; justify-content: center; min-height: 40px; border: 1px solid #27343f; border-radius: 8px; background: #17212b; color: #edf4f1; text-decoration: none; cursor: pointer; font: inherit; }
    .qpayd-note { margin: 0; padding: 0 18px 18px; color: #9fb0aa; font-size: 13px; }
    .qpayd-error { margin: 0; padding: 0 18px 18px; color: #ff7b72; font-size: 13px; }
    .qpayd-error-panel { padding-top: 18px; }
  `;
  document.head.append(style);
}
