(function () {
  const STORAGE_KEY = "site_lang";
  const DEFAULT_LANG = "en";
  const SUPPORTED = ["en", "pl"];
  const SUPPORTED_SET = new Set(SUPPORTED);

  function normalizeLanguage(language) {
    const value = String(language || "").toLowerCase().trim();
    if (!value) {
      return DEFAULT_LANG;
    }

    if (SUPPORTED_SET.has(value)) {
      return value;
    }

    const shortCode = value.split("-")[0];
    if (SUPPORTED_SET.has(shortCode)) {
      return shortCode;
    }

    return DEFAULT_LANG;
  }

  function detectBrowserLanguage() {
    const candidates = Array.isArray(window.navigator.languages) && window.navigator.languages.length
      ? window.navigator.languages
      : [window.navigator.language];

    for (const candidate of candidates) {
      const normalized = normalizeLanguage(candidate);
      if (SUPPORTED_SET.has(normalized)) {
        return normalized;
      }
    }

    return DEFAULT_LANG;
  }

  function readStoredLanguage() {
    try {
      return window.localStorage.getItem(STORAGE_KEY);
    } catch {
      return "";
    }
  }

  function getLanguage() {
    const stored = normalizeLanguage(readStoredLanguage());
    if (stored && readStoredLanguage()) {
      return stored;
    }
    return detectBrowserLanguage();
  }

  function setLanguage(language) {
    const normalized = normalizeLanguage(language);
    const previous = getLanguage();

    try {
      window.localStorage.setItem(STORAGE_KEY, normalized);
    } catch {
      // ignore storage failures
    }

    if (previous !== normalized) {
      window.dispatchEvent(new CustomEvent("site-language-change", {
        detail: { language: normalized }
      }));
    }

    return normalized;
  }

  function translate(dictionary, key, language) {
    const selectedLanguage = normalizeLanguage(language || getLanguage());
    const fallback = (dictionary.en && dictionary.en[key]) || "";

    if (dictionary[selectedLanguage] && Object.prototype.hasOwnProperty.call(dictionary[selectedLanguage], key)) {
      return dictionary[selectedLanguage][key];
    }

    return fallback;
  }

  function apply(dictionary, root, language) {
    const selectedLanguage = normalizeLanguage(language || getLanguage());
    const targetRoot = root || document;

    targetRoot.querySelectorAll("[data-i18n]").forEach((element) => {
      const key = element.getAttribute("data-i18n");
      element.textContent = translate(dictionary, key, selectedLanguage);
    });

    targetRoot.querySelectorAll("[data-i18n-placeholder]").forEach((element) => {
      const key = element.getAttribute("data-i18n-placeholder");
      element.setAttribute("placeholder", translate(dictionary, key, selectedLanguage));
    });

    targetRoot.querySelectorAll("[data-i18n-title]").forEach((element) => {
      const key = element.getAttribute("data-i18n-title");
      element.setAttribute("title", translate(dictionary, key, selectedLanguage));
    });

    targetRoot.querySelectorAll("[data-i18n-aria-label]").forEach((element) => {
      const key = element.getAttribute("data-i18n-aria-label");
      element.setAttribute("aria-label", translate(dictionary, key, selectedLanguage));
    });

    targetRoot.querySelectorAll("[data-i18n-value]").forEach((element) => {
      const key = element.getAttribute("data-i18n-value");
      element.value = translate(dictionary, key, selectedLanguage);
    });

    document.documentElement.setAttribute("lang", selectedLanguage);
    return selectedLanguage;
  }

  const LOGIN_ERRORS = {
    en: {
      access_denied: "Login cancelled. You did not authorize the Discord app.",
      fallback: "Logging in with Discord failed. Please try again.",
      close: "Close"
    },
    pl: {
      access_denied: "Logowanie anulowane. Aplikacja Discord nie została autoryzowana.",
      fallback: "Logowanie przez Discorda nie powiodło się. Spróbuj ponownie.",
      close: "Zamknij"
    }
  };

  // After a failed login the backend redirects back with ?oauth_error=<code>.
  // Show a readable message and clean the address bar.
  function showLoginError() {
    let params;
    try {
      params = new URLSearchParams(window.location.search);
    } catch {
      return;
    }

    const code = params.get("oauth_error");
    if (!code) {
      return;
    }

    params.delete("oauth_error");
    const query = params.toString();
    try {
      window.history.replaceState(null, "", window.location.pathname + (query ? `?${query}` : "") + window.location.hash);
    } catch {
      // ignore history failures
    }

    const texts = LOGIN_ERRORS[getLanguage()] || LOGIN_ERRORS.en;
    const banner = document.createElement("div");
    banner.setAttribute("role", "alert");
    banner.style.cssText = "position:fixed;top:0;left:0;right:0;z-index:1000;display:flex;gap:16px;align-items:center;"
      + "justify-content:center;padding:12px 16px;background:#000;color:#fff;border-bottom:1px solid #fff;"
      + "font:16px/1.4 system-ui,sans-serif;text-align:center;";

    const message = document.createElement("span");
    message.textContent = Object.prototype.hasOwnProperty.call(texts, code) ? texts[code] : texts.fallback;

    const close = document.createElement("button");
    close.type = "button";
    close.textContent = texts.close;
    close.style.cssText = "background:#000;color:#fff;border:1px solid #fff;padding:4px 12px;cursor:pointer;font:inherit;";
    close.addEventListener("click", () => banner.remove());

    banner.append(message, close);
    document.body.prepend(banner);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", showLoginError);
  } else {
    showLoginError();
  }

  window.SiteLang = {
    DEFAULT_LANG,
    SUPPORTED,
    apply,
    detectBrowserLanguage,
    getLanguage,
    normalizeLanguage,
    setLanguage,
    translate
  };
})();
