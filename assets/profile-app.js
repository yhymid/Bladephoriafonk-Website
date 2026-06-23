(function () {
  const AUTH_PUBLIC_PROFILE_ENDPOINT = "/auth/public-profile";
  const AUTH_PROFILE_MODE_ENDPOINT = "/auth/profile-mode";
  const AUTH_PROFILE_DELETE_ENDPOINT = "/auth/profile-delete";
  const AUTH_LOGOUT_ENDPOINT = "/auth/logout";
  const REFRESH_INTERVAL_MS = 60 * 1000;

  const state = {
    currentSlug: extractCurrentSlug(),
    isAuthenticated: false,
    isOwner: false,
    publicMode: false,
    publicUrl: "",
    publicStateKey: "loading_profile"
  };

  let languageBootstrapped = false;

  const I18N = {
    en: {
      page_title: "Bladephoriafonk | Profile",
      brand_kicker: "Discord Profile Hub",
      page_heading: "Profile",
      intro_copy: "Your Discord profile, public connections and quick account actions live here.",
      language_label: "Language",
      nav_home: "Home",
      nav_server: "Server",
      nav_admin: "Administration",
      nav_changelog: "Changelog",
      no_banner: "No Discord banner available",
      connections_title: "Connections",
      connections_copy: "Accounts visible in Discord settings appear here. Hidden or revoked ones are omitted from public mode.",
      empty_connections: "No connected accounts were found. Add them in Discord settings and refresh the profile.",
      info_title: "Profile Info",
      label_display_name: "Display Name:",
      label_username: "Username:",
      label_public_badges: "Public Badges:",
      label_banner_color: "Banner Color:",
      label_accent_color: "Accent Color:",
      guild_title: "Guild Snapshot",
      guild_copy: "Extra details from the preferred Discord server, when Discord exposes them.",
      label_guild_name: "Guild Name:",
      label_guild_nick: "Guild Nickname:",
      label_guild_bio: "Guild Bio:",
      label_last_update: "Last update: {date}",
      back_home: "Back to home",
      sign_in_action: "Sign in with Discord",
      loading_profile: "Loading profile...",
      login_required: "Sign in to create or manage your own profile.",
      profile_not_found: "This profile does not exist.",
      profile_private: "This profile is private.",
      update_error: "Profile update failed. Please try again in a moment.",
      owner_status_ready: "Profile loaded successfully.",
      owner_status_refreshed: "Profile refreshed just now.",
      owner_status_public_on: "Public mode is enabled. Your profile can be shared.",
      owner_status_public_off: "Public mode is disabled. Only you can view the full profile.",
      owner_status_link_copied: "Public profile link copied.",
      owner_status_link_copy_failed: "Could not copy the link automatically.",
      public_mode_label_on: "Public profile",
      public_mode_label_off: "Private profile",
      public_mode_public_view: "Public snapshot",
      btn_logout: "Log out",
      btn_delete_profile: "Delete profile",
      btn_refresh_profile: "Refresh profile",
      btn_refreshing: "Refreshing...",
      btn_deleting: "Deleting...",
      btn_public_mode_on: "Turn public mode off",
      btn_public_mode_off: "Turn public mode on",
      btn_copy_public_link: "Copy public link",
      btn_copy_public_link_off: "Public link unavailable",
      visible: "Visible",
      verified: "Verified",
      yes: "Yes",
      no: "No",
      unknown_user: "Unknown",
      unknown_connection: "Unknown",
      delete_confirm: "Delete the profile and log out? This also removes the generated public page."
    },
    pl: {
      page_title: "Bladephoriafonk | Profil",
      brand_kicker: "Panel profilu Discord",
      page_heading: "Profil",
      intro_copy: "Tutaj masz swoj profil Discord, publiczne polaczenia i szybkie akcje konta.",
      language_label: "Jezyk",
      nav_home: "Strona glowna",
      nav_server: "Serwer",
      nav_admin: "Administracja",
      nav_changelog: "Dziennik zmian",
      no_banner: "Brak bannera Discord",
      connections_title: "Polaczenia",
      connections_copy: "Widoczne konta z ustawien Discord pojawiaja sie tutaj. Ukryte i cofniete polaczenia nie trafiaja do trybu publicznego.",
      empty_connections: "Nie znaleziono polaczonych kont. Dodaj je w ustawieniach Discord i odswiez profil.",
      info_title: "Informacje o profilu",
      label_display_name: "Display Name:",
      label_username: "Username:",
      label_public_badges: "Public Badges:",
      label_banner_color: "Kolor bannera:",
      label_accent_color: "Kolor akcentu:",
      guild_title: "Migawka z serwera",
      guild_copy: "Dodatkowe szczegoly z wybranego serwera Discord, jesli Discord je udostepnia.",
      label_guild_name: "Nazwa serwera:",
      label_guild_nick: "Pseudonim na serwerze:",
      label_guild_bio: "Bio na serwerze:",
      label_last_update: "Ostatnia aktualizacja: {date}",
      back_home: "Wroc do strony glownej",
      sign_in_action: "Zaloguj sie przez Discord",
      loading_profile: "Ladowanie profilu...",
      login_required: "Zaloguj sie, aby utworzyc lub zarzadzac swoim profilem.",
      profile_not_found: "Ten profil nie istnieje.",
      profile_private: "Ten profil jest prywatny.",
      update_error: "Aktualizacja profilu nie powiodla sie. Sprobuj ponownie za chwile.",
      owner_status_ready: "Profil zaladowany poprawnie.",
      owner_status_refreshed: "Profil zostal odswiezony.",
      owner_status_public_on: "Tryb publiczny jest wlaczony. Mozesz udostepnic swoj profil.",
      owner_status_public_off: "Tryb publiczny jest wylaczony. Pelny profil widzisz tylko Ty.",
      owner_status_link_copied: "Link do publicznego profilu zostal skopiowany.",
      owner_status_link_copy_failed: "Nie udalo sie automatycznie skopiowac linku.",
      public_mode_label_on: "Profil publiczny",
      public_mode_label_off: "Profil prywatny",
      public_mode_public_view: "Publiczna migawka",
      btn_logout: "Wyloguj",
      btn_delete_profile: "Usun profil",
      btn_refresh_profile: "Odswiez profil",
      btn_refreshing: "Odswiezanie...",
      btn_deleting: "Usuwanie...",
      btn_public_mode_on: "Wylacz tryb publiczny",
      btn_public_mode_off: "Wlacz tryb publiczny",
      btn_copy_public_link: "Skopiuj link publiczny",
      btn_copy_public_link_off: "Link publiczny niedostepny",
      visible: "Widoczne",
      verified: "Zweryfikowane",
      yes: "Tak",
      no: "Nie",
      unknown_user: "Nieznany",
      unknown_connection: "Nieznane",
      delete_confirm: "Usunac profil i wylogowac? To usunie tez wygenerowana strone publiczna."
    }
  };

  function t(key) {
    return window.SiteLang.translate(I18N, key, window.SiteLang.getLanguage());
  }

  function formatMessage(key, params) {
    return window.SiteUi.format(t(key), params);
  }

  function currentLocale() {
    return window.SiteLang.getLanguage() === "pl" ? "pl-PL" : "en-US";
  }

  function extractCurrentSlug() {
    const segments = window.location.pathname.split("/").filter(Boolean);
    const profileIndex = segments.indexOf("profile");
    if (profileIndex === -1) {
      return "";
    }
    return segments[profileIndex + 1] || "";
  }

  function setText(id, value) {
    const element = document.getElementById(id);
    if (element) {
      element.textContent = value;
    }
  }

  function setHidden(id, hidden) {
    const element = document.getElementById(id);
    if (element) {
      element.hidden = hidden;
    }
  }

  function setOwnerStatus(message, tone) {
    const status = document.getElementById("owner-status");
    if (!status) {
      return;
    }
    status.textContent = message || "";
    status.dataset.tone = tone || "";
  }

  function setUpdated(value) {
    setText("profile-updated", formatMessage("label_last_update", { date: value }));
  }

  function formatDate(value) {
    const date = value ? new Date(value) : new Date();
    if (Number.isNaN(date.getTime())) {
      return new Date().toLocaleString(currentLocale());
    }
    return date.toLocaleString(currentLocale());
  }

  function toHexColor(numberValue) {
    if (typeof numberValue !== "number" || !Number.isFinite(numberValue)) {
      return "-";
    }
    return `#${numberValue.toString(16).padStart(6, "0")}`;
  }

  function decodeBadges(publicFlags) {
    if (typeof publicFlags !== "number" || publicFlags <= 0) {
      return "-";
    }

    const badges = [];
    if (publicFlags & 1) badges.push("Discord Employee");
    if (publicFlags & 2) badges.push("Partnered Server Owner");
    if (publicFlags & 4) badges.push("HypeSquad Events");
    if (publicFlags & 8) badges.push("Bug Hunter Level 1");
    if (publicFlags & 64) badges.push("House Bravery");
    if (publicFlags & 128) badges.push("House Brilliance");
    if (publicFlags & 256) badges.push("House Balance");
    if (publicFlags & 512) badges.push("Early Supporter");
    if (publicFlags & 16384) badges.push("Bug Hunter Level 2");
    if (publicFlags & 131072) badges.push("Verified Bot Developer");
    if (publicFlags & 4194304) badges.push("Active Developer");

    return badges.length ? badges.join(", ") : `Flags: ${publicFlags}`;
  }

  function setBanner(user) {
    const wrap = document.getElementById("profile-banner");
    const image = document.getElementById("profile-banner-image");
    const fallback = document.getElementById("profile-banner-fallback");
    const color = user.banner_color || "#111017";

    if (wrap) {
      wrap.style.background = `linear-gradient(135deg, ${color}, rgba(10, 9, 12, 0.92))`;
    }

    if (!image || !fallback) {
      return;
    }

    const bannerUrl = user.guild_banner_url || user.banner_url || "";
    if (bannerUrl) {
      image.src = bannerUrl;
      image.alt = `${user.global_name || user.username || t("unknown_user")} banner`;
      image.hidden = false;
      fallback.hidden = true;
      image.onerror = () => {
        image.hidden = true;
        fallback.hidden = false;
      };
      return;
    }

    image.hidden = true;
    image.removeAttribute("src");
    fallback.hidden = false;
  }

  function setAvatar(user) {
    const avatar = document.getElementById("profile-avatar");
    if (!avatar) {
      return;
    }

    avatar.src = user.avatar_url || "https://cdn.discordapp.com/embed/avatars/0.png";
    avatar.alt = `${user.global_name || user.username || t("unknown_user")} avatar`;
  }

  function buildConnectionIcon(type) {
    const icon = document.createElement("div");
    icon.className = "connection-pill";
    icon.textContent = (type || "?").slice(0, 1).toUpperCase();
    icon.title = type || t("unknown_connection");
    return icon;
  }

  function renderConnections(connections) {
    const icons = document.getElementById("connection-icons");
    const list = document.getElementById("oauth-connections");
    const empty = document.getElementById("connections-empty");

    if (!icons || !list || !empty) {
      return;
    }

    icons.innerHTML = "";
    list.innerHTML = "";

    if (!Array.isArray(connections) || connections.length === 0) {
      empty.hidden = false;
      return;
    }

    empty.hidden = true;

    connections.forEach((connection) => {
      icons.appendChild(buildConnectionIcon(connection.type));

      const card = document.createElement("article");
      card.className = "connection-card";

      const head = document.createElement("div");
      head.className = "connection-head";

      const name = document.createElement("strong");
      name.textContent = connection.name || t("unknown_connection");

      const type = document.createElement("span");
      type.className = "connection-type";
      type.textContent = connection.type || "unknown";

      const meta = document.createElement("p");
      meta.className = "connection-meta";
      meta.textContent = `${t("visible")}: ${connection.visibility === 1 ? t("yes") : t("no")} | ${t("verified")}: ${connection.verified ? t("yes") : t("no")}`;

      head.appendChild(name);
      head.appendChild(type);
      card.appendChild(head);
      card.appendChild(meta);
      list.appendChild(card);
    });
  }

  function setModeIndicator(mode) {
    const indicator = document.getElementById("profile-mode-indicator");
    if (!indicator) {
      return;
    }

    if (mode === "public-view") {
      indicator.textContent = t("public_mode_public_view");
      indicator.dataset.tone = "info";
      return;
    }

    if (mode === "public-on") {
      indicator.textContent = t("public_mode_label_on");
      indicator.dataset.tone = "success";
      return;
    }

    indicator.textContent = t("public_mode_label_off");
    indicator.dataset.tone = "warning";
  }

  function setCopyButtonState() {
    const copyButton = document.getElementById("copy-public-link");
    if (!copyButton) {
      return;
    }

    const enabled = state.isOwner && state.publicMode && Boolean(state.publicUrl);
    copyButton.disabled = !enabled;
    copyButton.textContent = enabled ? t("btn_copy_public_link") : t("btn_copy_public_link_off");
  }

  function refreshOwnerControlTexts() {
    setText("logout-discord", t("btn_logout"));
    setText("refresh-profile", t("btn_refresh_profile"));
    setText("delete-profile", t("btn_delete_profile"));

    const toggle = document.getElementById("public-mode-toggle");
    if (toggle) {
      toggle.textContent = state.publicMode ? t("btn_public_mode_on") : t("btn_public_mode_off");
    }

    setCopyButtonState();
  }

  function setOwnerControlsVisible(isVisible) {
    const ownerControls = document.getElementById("owner-controls");
    if (!ownerControls) {
      return;
    }
    ownerControls.hidden = !isVisible;
  }

  function setGuildPanel(user) {
    const guildPanel = document.getElementById("guild-panel");
    const hasGuildData = Boolean(user.guild_name || user.guild_nick || user.guild_bio);

    if (guildPanel) {
      guildPanel.hidden = !hasGuildData;
    }

    setText("guild-name", user.guild_name || "-");
    setText("guild-nick", user.guild_nick || "-");
    setText("guild-bio", user.guild_bio || "-");
  }

  function applyProfile(user, connections, updatedAt, options = {}) {
    const displayName = user.global_name || user.username || t("unknown_user");
    const username = user.username ? `@${user.username}` : "@-";
    const isPublicView = Boolean(options.publicView);

    setBanner(user);
    setAvatar(user);
    renderConnections(connections);
    setGuildPanel(user);

    setText("profile-display-name", displayName);
    setText("profile-username", username);
    setText("profile-description", isPublicView ? t("connections_copy") : t("intro_copy"));
    setText("info-display-name", displayName);
    setText("info-username", username);
    setText("info-public-badges", decodeBadges(user.public_flags));
    setText("info-banner-color", user.banner_color || "-");
    setText("info-accent-color", toHexColor(user.accent_color));
    setUpdated(formatDate(updatedAt));
  }

  function showPublicState(key, options = {}) {
    state.publicStateKey = key;
    setHidden("profile-main", true);
    setHidden("public-state", false);
    setText("public-state-message", t(key));
    setHidden("public-state-home-link", options.showHome === false);
    setHidden("public-state-login-link", !options.showLogin);
  }

  function hidePublicState() {
    setHidden("public-state", true);
    setHidden("profile-main", false);
  }

  function resetProfile() {
    setBanner({ banner_color: "#111017" });
    setAvatar({});
    renderConnections([]);
    setGuildPanel({});
    setText("profile-display-name", t("page_heading"));
    setText("profile-username", "@-");
    setText("profile-description", t("intro_copy"));
    setText("info-display-name", "-");
    setText("info-username", "-");
    setText("info-public-badges", "-");
    setText("info-banner-color", "-");
    setText("info-accent-color", "-");
    setUpdated(formatDate());
  }

  async function copyPublicLink() {
    if (!state.publicUrl) {
      setOwnerStatus(t("owner_status_link_copy_failed"), "danger");
      return;
    }

    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(state.publicUrl);
      } else {
        throw new Error("clipboard_unavailable");
      }
      setOwnerStatus(t("owner_status_link_copied"), "success");
    } catch {
      setOwnerStatus(t("owner_status_link_copy_failed"), "danger");
    }
  }

  async function showPublicView(slug) {
    try {
      const query = new URLSearchParams({ slug });
      const response = await fetch(`${AUTH_PUBLIC_PROFILE_ENDPOINT}?${query.toString()}`, {
        cache: "no-store"
      });

      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        if (response.status === 403) {
          showPublicState("profile_private", { showLogin: true });
          return;
        }
        showPublicState("profile_not_found", { showLogin: true });
        return;
      }

      hidePublicState();
      applyProfile(data.profile?.user || {}, data.profile?.connections || [], data.profile?.updated_at, {
        publicView: true
      });
      setModeIndicator("public-view");
      setOwnerControlsVisible(false);
      setOwnerStatus("", "");
    } catch {
      showPublicState("profile_not_found", { showLogin: true });
    }
  }

  async function setPublicMode(enabled) {
    const toggle = document.getElementById("public-mode-toggle");
    if (!toggle) {
      return;
    }

    toggle.disabled = true;
    try {
      const response = await fetch(AUTH_PROFILE_MODE_ENDPOINT, {
        method: "POST",
        credentials: "include",
        headers: {
          "content-type": "application/json"
        },
        body: JSON.stringify({ public_mode: Boolean(enabled) })
      });

      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `HTTP ${response.status}`);
      }

      state.publicMode = Boolean(data.profile?.public_mode);
      setModeIndicator(state.publicMode ? "public-on" : "private-off");
      refreshOwnerControlTexts();
      setOwnerStatus(state.publicMode ? t("owner_status_public_on") : t("owner_status_public_off"), "success");
    } catch {
      setOwnerStatus(t("update_error"), "danger");
    } finally {
      toggle.disabled = false;
    }
  }

  async function deleteProfile() {
    const button = document.getElementById("delete-profile");
    if (!button) {
      return;
    }

    if (!window.confirm(t("delete_confirm"))) {
      return;
    }

    const previousText = button.textContent;
    button.disabled = true;
    button.textContent = t("btn_deleting");

    try {
      const response = await fetch(AUTH_PROFILE_DELETE_ENDPOINT, {
        method: "POST",
        credentials: "include"
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `HTTP ${response.status}`);
      }
      window.location.href = "/";
    } catch {
      button.disabled = false;
      button.textContent = previousText;
      setOwnerStatus(t("update_error"), "danger");
    }
  }

  async function logout() {
    try {
      await fetch(AUTH_LOGOUT_ENDPOINT, {
        method: "POST",
        credentials: "include"
      });
    } catch {
      // ignore
    }
    window.location.href = "/";
  }

  function bindControls() {
    document.getElementById("logout-discord")?.addEventListener("click", () => {
      void logout();
    });

    document.getElementById("refresh-profile")?.addEventListener("click", async (event) => {
      const button = event.currentTarget;
      if (!(button instanceof HTMLButtonElement)) {
        return;
      }

      button.disabled = true;
      button.textContent = t("btn_refreshing");
      await loadSession({ force: true, fromManualRefresh: true });
      button.disabled = false;
      button.textContent = t("btn_refresh_profile");
    });

    document.getElementById("public-mode-toggle")?.addEventListener("click", () => {
      void setPublicMode(!state.publicMode);
    });

    document.getElementById("copy-public-link")?.addEventListener("click", () => {
      void copyPublicLink();
    });

    document.getElementById("delete-profile")?.addEventListener("click", () => {
      void deleteProfile();
    });
  }

  async function loadSession(options = {}) {
    try {
      const data = await window.SiteUi.fetchSession({ force: Boolean(options.force) });

      if (!data.authenticated) {
        state.isAuthenticated = false;
        state.isOwner = false;
        state.publicMode = false;
        state.publicUrl = "";
        setOwnerControlsVisible(false);
        refreshOwnerControlTexts();

        if (state.currentSlug) {
          await showPublicView(state.currentSlug);
        } else {
          resetProfile();
          showPublicState("login_required", { showLogin: true });
        }
        return;
      }

      state.isAuthenticated = true;
      const ownSlug = String(data.profile?.slug || "");
      const ownUrl = String(data.profile?.url || "");
      state.publicMode = Boolean(data.profile?.public_mode);
      state.publicUrl = ownSlug ? `${window.location.origin}${ownUrl || `/profile/${ownSlug}/`}` : "";

      if (ownSlug && state.currentSlug && ownSlug !== state.currentSlug) {
        window.location.href = `/profile/${ownSlug}/`;
        return;
      }

      const isOwner = !state.currentSlug || (ownSlug && state.currentSlug === ownSlug);
      state.isOwner = Boolean(isOwner);

      if (!isOwner && state.currentSlug) {
        setOwnerControlsVisible(false);
        await showPublicView(state.currentSlug);
        return;
      }

      hidePublicState();
      applyProfile(data.user || {}, Array.isArray(data.connections) ? data.connections : [], data.updated_at);
      setOwnerControlsVisible(true);
      setModeIndicator(state.publicMode ? "public-on" : "private-off");
      refreshOwnerControlTexts();
      setOwnerStatus(
        options.fromManualRefresh
          ? t("owner_status_refreshed")
          : t("owner_status_ready"),
        "success"
      );
    } catch {
      setOwnerControlsVisible(false);
      if (state.currentSlug) {
        showPublicState("profile_not_found", { showLogin: true });
      } else {
        resetProfile();
        showPublicState("update_error", { showLogin: true });
      }
    }
  }

  function applyLanguage() {
    window.SiteUi.applyLanguage(I18N, {
      onApplied() {
        refreshOwnerControlTexts();
        setText("public-state-message", t(state.publicStateKey));
        setUpdated(formatDate());
        if (state.isOwner) {
          setModeIndicator(state.publicMode ? "public-on" : "private-off");
        }
        if (languageBootstrapped && (state.isAuthenticated || state.currentSlug)) {
          void loadSession();
        }
      }
    });

    const loginLink = document.getElementById("public-state-login-link");
    if (loginLink) {
      loginLink.href = `/auth/discord/login?return_to=${encodeURIComponent(state.currentSlug ? `/profile/${state.currentSlug}/` : "/profile/")}`;
    }
  }

  async function bootstrap() {
    applyLanguage();
    bindControls();
    resetProfile();
    showPublicState("loading_profile", { showLogin: false, showHome: false });
    languageBootstrapped = true;
    await loadSession();
    window.SiteUi.attachVisibilityInterval(() => loadSession(), REFRESH_INTERVAL_MS);
  }

  bootstrap();
})();
