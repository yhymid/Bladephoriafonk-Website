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
