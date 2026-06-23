(function () {
  const AUTH_SESSION_ENDPOINT = "/auth/session";
  const AUTH_LOGOUT_ENDPOINT = "/auth/logout";

  function byId(id) {
    return id ? document.getElementById(id) : null;
  }

  function escapeReturnTo(input) {
    const fallback = "/";
    const value = String(input || "").trim();
    if (!value || !value.startsWith("/") || value.startsWith("//") || value.includes("://")) {
      return fallback;
    }
    return value;
  }

  function toggleElement(element, isVisible) {
    if (!element) {
      return;
    }
    element.hidden = !isVisible;
  }

  function format(template, params) {
    let value = String(template || "");
    Object.entries(params || {}).forEach(([key, paramValue]) => {
      value = value.replace(new RegExp(`\\{${key}\\}`, "g"), String(paramValue));
    });
    return value;
  }

  function setStatusText(statusElement, message, tone) {
    if (!statusElement) {
      return;
    }

    statusElement.textContent = message || "";
    statusElement.dataset.tone = tone || "";
    statusElement.hidden = !message;
  }

  function applyLanguage(dictionary, options = {}) {
    const select = byId(options.selectId || "language-select");
    const titleKey = options.titleKey || "page_title";

    function render(language) {
      const selectedLanguage = window.SiteLang.apply(dictionary, document, language);
      if (select) {
        select.value = selectedLanguage;
      }
      if (titleKey) {
        document.title = window.SiteLang.translate(dictionary, titleKey, selectedLanguage);
      }
      if (typeof options.onApplied === "function") {
        options.onApplied(selectedLanguage);
      }
      return selectedLanguage;
    }

    if (select && !select.dataset.siteUiBound) {
      select.dataset.siteUiBound = "true";
      select.value = window.SiteLang.getLanguage();
      select.addEventListener("change", () => {
        render(window.SiteLang.setLanguage(select.value));
      });
    }

    const listener = (event) => {
      render(event?.detail?.language);
    };
    window.addEventListener("site-language-change", listener);
    render(window.SiteLang.getLanguage());

    return {
      destroy() {
        window.removeEventListener("site-language-change", listener);
      },
      render
    };
  }

  async function fetchSession(options = {}) {
    const query = new URLSearchParams();
    if (options.force) {
      query.set("refresh", "1");
    }

    const response = await fetch(`${AUTH_SESSION_ENDPOINT}${query.toString() ? `?${query}` : ""}`, {
      cache: "no-store",
      credentials: "include"
    });

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }

    return response.json();
  }

  function initAuthControls(options = {}) {
    const loginEl = byId(options.loginId || "discord-login");
    const profileEl = byId(options.profileId || "my-profile");
    const logoutEl = byId(options.logoutId || "discord-logout");
    const statusEl = byId(options.statusId || "auth-status");
    const returnTo = escapeReturnTo(options.returnTo || `${window.location.pathname}${window.location.search}`);

    if (loginEl) {
      loginEl.href = `/auth/discord/login?return_to=${encodeURIComponent(returnTo)}`;
    }

    if (logoutEl && !logoutEl.dataset.siteUiBound) {
      logoutEl.dataset.siteUiBound = "true";
      logoutEl.addEventListener("click", async () => {
        setStatusText(statusEl, options.loggingOutMessage || "", "info");
        try {
          await fetch(AUTH_LOGOUT_ENDPOINT, {
            method: "POST",
            credentials: "include"
          });
        } catch {
          // ignore logout failures and still refresh UI
        }

        await refresh();
        if (typeof options.onLogout === "function") {
          options.onLogout();
        }
      });
    }

    function render(data, requestFailed) {
      const authenticated = Boolean(data && data.authenticated);
      const profileUrl = data?.profile?.url || "/profile/";

      toggleElement(loginEl, !authenticated);
      toggleElement(profileEl, authenticated);
      toggleElement(logoutEl, authenticated);

      if (profileEl) {
        profileEl.href = profileUrl;
      }

      if (typeof options.renderSession === "function") {
        options.renderSession(data, requestFailed);
      }
    }

    async function refresh(refreshOptions = {}) {
      try {
        const data = await fetchSession(refreshOptions);
        render(data, false);
        if (typeof options.onSession === "function") {
          options.onSession(data, null);
        }
        return data;
      } catch (error) {
        render({ authenticated: false }, true);
        if (typeof options.onSession === "function") {
          options.onSession({ authenticated: false }, error);
        }
        return { authenticated: false, error: String(error) };
      }
    }

    return {
      refresh,
      setStatus(message, tone) {
        setStatusText(statusEl, message, tone);
      }
    };
  }

  function attachVisibilityInterval(handler, intervalMs) {
    let timerId = null;

    const tick = async () => {
      if (document.hidden) {
        return;
      }
      await handler();
    };

    function start() {
      stop();
      timerId = window.setInterval(() => {
        void tick();
      }, intervalMs);
    }

    function stop() {
      if (timerId !== null) {
        window.clearInterval(timerId);
        timerId = null;
      }
    }

    const onVisibilityChange = () => {
      if (!document.hidden) {
        void tick();
      }
    };

    document.addEventListener("visibilitychange", onVisibilityChange);
    start();

    return {
      start,
      stop,
      tick,
      destroy() {
        stop();
        document.removeEventListener("visibilitychange", onVisibilityChange);
      }
    };
  }

  window.SiteUi = {
    applyLanguage,
    attachVisibilityInterval,
    fetchSession,
    format,
    initAuthControls
  };
})();
