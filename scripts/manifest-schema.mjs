// Manifest + storefront metadata validation shared by validate-manifests,
// build-packages and build-catalog. One schema, one implementation — the
// catalog build must not accept a manifest the quality gate rejected.
import { statSync } from "node:fs";
import path from "node:path";

export const STORE_CATEGORIES_MAX = 8;
export const DATA_COMPATIBILITY_MAX = 16;

const KINDS = new Set(["source", "bridge"]);
const INJECTION_KINDS = new Set(["basic", "cookies", "header", "json"]);
const SETTING_KINDS = new Set(["username", "text", "api_key", "token", "secret"]);
const SECRET_SETTING_KINDS = new Set(["api_key", "token", "secret"]);
const ROLES = new Set(["read", "edit", "import", "export", "sync"]);
const FIDELITIES = new Set(["native", "lossless", "lossy", "metadata-only"]);
const PLATFORMS = new Set(["windows", "macos", "linux", "ios", "android"]);
const PUBLISHER_TIERS = new Set(["kosmos", "verified", "community"]);
const SAFE_KEY = /^[a-z][a-z0-9_]{0,63}$/;
const SAFE_CATEGORY = /^[a-z][a-z0-9-]{0,63}$/;
// The id is embedded in `.kspkg` artifact names, `icon-<id>.png` assets and
// release URLs — path separators, whitespace or case would break those
// consumers or escape the output directory.
const SAFE_ID = /^[a-z0-9][a-z0-9._-]{0,63}$/;

export function fail(message) {
  throw new Error(message);
}

function object(value) {
  return value && Object.prototype.toString.call(value) === "[object Object]";
}

function httpsUrl(value) {
  try {
    return new URL(value).protocol === "https:";
  } catch {
    return false;
  }
}

export function networkOrigins(manifest, label, { required } = {}) {
  const network = manifest.permissions.find((item) => item?.capability === "network");
  if (!network) {
    if (required) fail(`${label}: HTTPS network permission is required`);
    return new Set();
  }
  if (!Array.isArray(network.scopes) || network.scopes.length === 0) {
    fail(`${label}: HTTPS network permission is required`);
  }
  return new Set(
    network.scopes.map((scope) => {
      if (typeof scope !== "string" || !httpsUrl(scope)) fail(`${label}: invalid network scope`);
      return new URL(scope).origin;
    }),
  );
}

function validateDataCompatibility(entries, label) {
  if (!Array.isArray(entries) || entries.length > DATA_COMPATIBILITY_MAX) {
    fail(`${label}: invalid data_compatibility`);
  }
  const seen = new Set();
  for (const entry of entries) {
    if (
      !object(entry) ||
      typeof entry.type !== "string" ||
      !entry.type ||
      typeof entry.versions !== "string" ||
      !entry.versions ||
      !Array.isArray(entry.roles) ||
      entry.roles.length === 0 ||
      entry.roles.some((role) => !ROLES.has(role)) ||
      typeof entry.via !== "string" ||
      !entry.via ||
      !FIDELITIES.has(entry.fidelity) ||
      seen.has(`${entry.type}@${entry.versions}`)
    ) {
      fail(`${label}: invalid data_compatibility entry`);
    }
    seen.add(`${entry.type}@${entry.versions}`);
  }
}

function validateStore(store, manifest, externalIds, label) {
  if (!object(store)) fail(`${label}: store metadata is required`);
  if (
    typeof store.description !== "string" ||
    !store.description ||
    store.description.length > 4096 ||
    [...store.description].some((char) => char < " ")
  ) {
    fail(`${label}: invalid store.description`);
  }
  if (
    !Array.isArray(store.categories) ||
    store.categories.length === 0 ||
    store.categories.length > STORE_CATEGORIES_MAX ||
    new Set(store.categories).size !== store.categories.length ||
    store.categories.some((category) => !SAFE_CATEGORY.test(category))
  ) {
    fail(`${label}: invalid store.categories`);
  }
  if (store.connects_to !== undefined && store.connects_to !== null) {
    if (typeof store.connects_to !== "string" || !externalIds.has(store.connects_to)) {
      fail(`${label}: store.connects_to must reference an external app`);
    }
  }
  if (store.data_compatibility !== undefined) {
    validateDataCompatibility(store.data_compatibility, label);
  }
}

export function validateManifest(manifest, { packageDir, externalIds = new Set() }) {
  const label = manifest?.id ?? packageDir ?? "manifest";
  if (
    !object(manifest) ||
    manifest.schema_version !== 2 ||
    typeof manifest.id !== "string" ||
    !SAFE_ID.test(manifest.id) ||
    typeof manifest.version !== "string" ||
    !/^\d+\.\d+\.\d+$/.test(manifest.version) ||
    !KINDS.has(manifest.kind) ||
    manifest.engine_api !== ">=1.0.0" ||
    manifest.publisher !== "kosmos" ||
    typeof manifest.name !== "string" ||
    !manifest.name ||
    manifest.name.length > 128 ||
    typeof manifest.entrypoint !== "string" ||
    !manifest.entrypoint.endsWith(".exe") ||
    manifest.entrypoint.includes("/") ||
    manifest.entrypoint.includes("\\") ||
    manifest.icon !== "icon.png"
  ) {
    fail(`${label}: invalid manifest identity`);
  }
  if (!Array.isArray(manifest.permissions)) fail(`${label}: permissions are required`);
  const capabilities = new Set();
  for (const item of manifest.permissions) {
    const capability = object(item) ? item.capability : undefined;
    if (typeof capability !== "string" || !capability || capabilities.has(capability)) {
      fail(`${label}: invalid or duplicate permission capability`);
    }
    capabilities.add(capability);
  }
  // Source workers phone home to a service API — an HTTPS network permission
  // is mandatory there; bridges sync local files and legitimately have none,
  // but a bridge that does declare one is held to the same HTTPS-only rule.
  const origins = networkOrigins(manifest, label, { required: manifest.kind === "source" });
  const ark = manifest.permissions.find((item) => item?.capability === "ark.write");
  if (
    !Array.isArray(ark?.scopes) ||
    ark.scopes.length === 0 ||
    ark.scopes.some((scope) => typeof scope !== "string" || !scope)
  ) {
    fail(`${label}: ark.write permission is required`);
  }
  if (!object(manifest.data) || !Array.isArray(manifest.data.access) ||
      !Array.isArray(manifest.data.defines) || !Array.isArray(manifest.data.mappings)) {
    fail(`${label}: invalid data contract`);
  }
  const integration = manifest.integration;
  if (manifest.kind === "source") {
    if (!object(integration) || !Array.isArray(integration.settings) || integration.settings.length === 0) {
      fail(`${label}: integration settings are required`);
    }
    const secretKeys = new Set();
    const settingKeys = new Set();
    for (const setting of integration.settings) {
      if (
        !object(setting) ||
        !SAFE_KEY.test(setting.key) ||
        typeof setting.label !== "string" ||
        !SETTING_KINDS.has(setting.kind) ||
        typeof setting.required !== "boolean"
      ) {
        fail(`${label}: invalid integration setting`);
      }
      if (settingKeys.has(setting.key)) fail(`${label}: duplicate integration setting key`);
      settingKeys.add(setting.key);
      if (SECRET_SETTING_KINDS.has(setting.kind)) {
        secretKeys.add(setting.key);
        if (!object(setting.injection) || !INJECTION_KINDS.has(setting.injection.kind)) {
          fail(`${label}: invalid secret injection`);
        }
        if (
          !Array.isArray(setting.injection.origins) ||
          setting.injection.origins.length === 0 ||
          setting.injection.origins.some((origin) => {
            try {
              return !origins.has(new URL(origin).origin);
            } catch {
              return true;
            }
          })
        ) {
          fail(`${label}: secret origins must be covered by network permission`);
        }
      } else if (setting.injection !== undefined) {
        fail(`${label}: non-secret settings cannot declare an injection`);
      }
    }
    if (integration.login !== undefined) {
      const login = integration.login;
      const huawei = object(login) && login.code_exchange === "huawei_health";
      if (
        !object(login) ||
        typeof login.start_url !== "string" ||
        typeof login.completion_url !== "string" ||
        !login.start_url.startsWith("https://") ||
        (!huawei && !login.completion_url.startsWith("https://")) ||
        !Array.isArray(login.allowed_cookie_names) ||
        (!huawei && login.allowed_cookie_names.length === 0) ||
        login.allowed_cookie_names.some((name) => typeof name !== "string" || !name) ||
        !secretKeys.has(login.secret_setting)
      ) {
        fail(`${label}: invalid browser login contract`);
      }
      const loginSetting = integration.settings.find((setting) => setting.key === login.secret_setting);
      if (
        !loginSetting ||
        (huawei
          ? login.start_url !== "https://oauth-login.cloud.huawei.com/oauth2/v3/authorize" ||
            login.completion_url !== "hms://redirect_url" ||
            login.allowed_cookie_names.length !== 0 ||
            loginSetting.injection.kind !== "json"
          : loginSetting.injection.kind !== "cookies")
      ) {
        fail(`${label}: invalid login injection`);
      }
    }
    if (
      !object(integration.schedule) ||
      !Number.isSafeInteger(integration.schedule.interval_seconds) ||
      integration.schedule.interval_seconds <= 0
    ) {
      fail(`${label}: integration schedule is required`);
    }
  }
  if (
    !Array.isArray(manifest.targets) ||
    !manifest.targets.some(
      (item) => item?.runtime === "worker" && Array.isArray(item.os) && item.os.includes("windows"),
    )
  ) {
    fail(`${label}: Windows worker target is required`);
  }
  validateStore(manifest.store, manifest, externalIds, label);
  if (packageDir) {
    for (const file of ["Cargo.toml", manifest.icon]) {
      const full = path.join(packageDir, file);
      const info = statSync(full, { throwIfNoEntry: false });
      if (!info?.isFile() || info.size === 0) fail(`${label}: ${file} is missing or empty`);
    }
  }
  return manifest;
}

export function validateExternalApps(document) {
  if (!object(document) || document.schema_version !== 1 || !Array.isArray(document.external_apps)) {
    fail("external-apps: schema_version 1 document with external_apps is required");
  }
  const ids = new Set();
  for (const app of document.external_apps) {
    if (
      !object(app) ||
      typeof app.id !== "string" ||
      !app.id.startsWith("external.") ||
      ids.has(app.id) ||
      typeof app.name !== "string" ||
      !app.name ||
      typeof app.publisher !== "string" ||
      !app.publisher ||
      !PUBLISHER_TIERS.has(app.publisher_tier) ||
      typeof app.description !== "string" ||
      !app.description ||
      !Array.isArray(app.categories) ||
      app.categories.some((category) => !SAFE_CATEGORY.test(category)) ||
      !Array.isArray(app.platforms) ||
      app.platforms.length === 0 ||
      app.platforms.some((platform) => !PLATFORMS.has(platform)) ||
      !httpsUrl(app.official_url) ||
      (app.icon_url !== null && !httpsUrl(app.icon_url))
    ) {
      fail(`external-apps: invalid entry ${app?.id ?? "?"}`);
    }
    validateDataCompatibility(app.data_compatibility ?? [], `external-apps ${app.id}`);
    ids.add(app.id);
  }
  return ids;
}
