class DashboardClient {
  constructor(root) {
    this.root = root;
    this.token = "";
    this.selectedNamespace = "";
    this.namespaceList = root.querySelector("[data-namespace-list]");
    this.stateView = root.querySelector("[data-state-view]");
    this.status = root.querySelector("[data-status]");
    this.selectedNamespaceLabel = root.querySelector("[data-selected-namespace]");
    this.configNamespace = root.querySelector("[data-config-namespace]");
    this.configRevision = root.querySelector("[data-config-revision]");
    this.configPayload = root.querySelector("[data-config-payload]");
  }

  start() {
    this.root.querySelector("[data-token-form]").addEventListener("submit", (event) => {
      event.preventDefault();
      const token = new FormData(event.currentTarget).get("token");
      this.token = typeof token === "string" ? token.trim() : "";
      this.setStatus(this.token ? "Admin token loaded for API calls." : "Token cleared.");
      this.loadRuntimeState();
    });

    this.root.querySelector("[data-clear-token]").addEventListener("click", () => {
      this.token = "";
      this.root.querySelector("#admin-token").value = "";
      this.setStatus("Token cleared. Loopback-only access will still work when enabled.");
      this.loadRuntimeState();
    });

    this.root.querySelector("[data-refresh]").addEventListener("click", () => {
      this.loadRuntimeState();
    });

    this.root.querySelector("[data-config-form]").addEventListener("submit", (event) => {
      event.preventDefault();
      this.applyConfig();
    });

    this.loadRuntimeState();
  }

  headers(jsonBody = false) {
    const headers = { Accept: "application/json" };
    if (jsonBody) {
      headers["Content-Type"] = "application/json";
    }
    if (this.token) {
      headers.Authorization = `Bearer ${this.token}`;
    }
    return headers;
  }

  async request(path, options = {}) {
    const response = await fetch(path, {
      ...options,
      headers: {
        ...this.headers(Boolean(options.body)),
        ...(options.headers || {}),
      },
    });
    const text = await response.text();
    if (!response.ok) {
      throw new Error(this.errorMessage(response.status, text));
    }
    if (!text) {
      return null;
    }
    return JSON.parse(text);
  }

  errorMessage(status, text) {
    if (!text) {
      return `Request failed with HTTP ${status}.`;
    }
    try {
      const parsed = JSON.parse(text);
      return parsed.error?.message || parsed.error || `Request failed with HTTP ${status}.`;
    } catch (_error) {
      return text;
    }
  }

  async loadRuntimeState() {
    try {
      this.setStatus("Loading runtime state...");
      const state = await this.request("/admin/state");
      this.renderNamespaces(state.namespaces || []);
      this.setStatus("Runtime state loaded.");
    } catch (error) {
      this.namespaceList.innerHTML = "";
      this.setStatus(error.message, true);
    }
  }

  renderNamespaces(namespaces) {
    this.namespaceList.innerHTML = "";
    if (!namespaces.length) {
      this.namespaceList.textContent = "No namespaces are configured yet.";
      return;
    }

    for (const item of namespaces) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "namespace-card";
      button.setAttribute("aria-pressed", String(item.namespace === this.selectedNamespace));
      button.innerHTML = `
        <strong>${this.escapeHtml(item.namespace)}</strong>
        <span>${item.upstream_count} upstreams - ${item.model_alias_count} aliases</span>
        <span>revision ${this.escapeHtml(item.revision)}</span>
      `;
      button.addEventListener("click", () => this.loadNamespace(item.namespace));
      this.namespaceList.appendChild(button);
    }
  }

  async loadNamespace(namespace) {
    try {
      this.selectedNamespace = namespace;
      this.selectedNamespaceLabel.textContent = namespace;
      this.configNamespace.value = namespace;
      this.setStatus(`Loading namespace ${namespace}...`);
      const state = await this.request(`/admin/namespaces/${encodeURIComponent(namespace)}/state`);
      this.configRevision.value = state.revision || "";
      this.stateView.textContent = JSON.stringify(state, null, 2);
      this.setStatus(`Namespace ${namespace} loaded. Redacted state is read-only.`);
      this.loadRuntimeState();
    } catch (error) {
      this.setStatus(error.message, true);
    }
  }

  async applyConfig() {
    const namespace = this.configNamespace.value.trim();
    if (!namespace) {
      this.setStatus("Namespace is required.", true);
      return;
    }

    let config;
    try {
      config = JSON.parse(this.configPayload.value);
    } catch (error) {
      this.setStatus(`Runtime config payload is not valid JSON: ${error.message}`, true);
      return;
    }

    const request = { config };
    const revision = this.configRevision.value.trim();
    if (revision) {
      request.if_revision = revision;
    }

    try {
      this.setStatus(`Applying config to ${namespace}...`);
      const result = await this.request(`/admin/namespaces/${encodeURIComponent(namespace)}/config`, {
        method: "POST",
        body: JSON.stringify(request),
      });
      this.configRevision.value = result.revision || "";
      this.setStatus(`Config applied to ${namespace}. New revision: ${result.revision}.`);
      await this.loadNamespace(namespace);
    } catch (error) {
      this.setStatus(error.message, true);
    }
  }

  setStatus(message, isError = false) {
    this.status.textContent = message;
    this.status.classList.toggle("error", isError);
  }

  escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, (char) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    })[char]);
  }
}

const root = document.querySelector("[data-dashboard-root]");
if (root) {
  new DashboardClient(root).start();
}
