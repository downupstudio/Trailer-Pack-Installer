import json
import time
import requests

print("=" * 60)
print("Environment check")
print("=" * 60)
print(f"  requests.__version__ : {getattr(requests, '__version__', 'MISSING')}")

MODRINTH_API = "https://api.modrinth.com/v2"
FABRIC_META_API = "https://meta.fabricmc.net/v2"

SUPPORTED_VERSIONS = [
    "1.21.1", "1.21.4", "1.21.5", "1.21.6",
    "1.21.7", "1.21.8", "1.21.9", "1.21.10", "1.21.11",
]

CIT_RESEWN_CUTOFF = "1.21.5"
EATING_ANIMATION_MOD_CUTOFF = "1.21.4"

MOD_SLUGS = [
    "fabric-api", "iris", "sodium", "entity-model-features",
    "entitytexturefeatures", "continuity",
]

SHADER_SLUGS = ["complementary-unbound"]

RESOURCE_PACK_SLUGS = [
    "bare-bones", "bare-bones-better-leaves", "bare-bones-connected-glass",
    "bare-bones-x-eating-animation", "bare-bones-low-on-fire",
    "bare-bones-x-fresh-animations", "better-lanterns", "cubic-sun-moon",
    "even-better-enchants", "fresh-animations", "low-shield-pack",
    "new-glowing-ores", "visual-armor-trims",
]

EATING_ANIMATION_PACK_SLUG = "theones-eating-animation-pack"

HEADERS = {"User-Agent": "TrailerPackManifestGenerator/1.0 (contact: you@example.com)"}
REQUEST_DELAY = 0.15


def version_tuple(v):
    return tuple(int(x) for x in v.split("."))


def _get_status(resp):
    for attr in ("status_code", "status"):
        if hasattr(resp, attr):
            val = getattr(resp, attr)
            if val is not None:
                return int(val)
    return None


def api_get(url, params=None, retries=3):
    for attempt in range(retries):
        try:
            resp = requests.get(url, params=params, headers=HEADERS, timeout=30)
        except Exception as exc:
            if attempt == retries - 1:
                print(f"  Request exception: {exc!r}")
                return None
            time.sleep(2 ** attempt)
            continue

        status = _get_status(resp)
        if status == 429:
            wait = int(resp.headers.get("Retry-After", 5))
            time.sleep(wait)
            continue
        if status is None or status >= 400:
            print(f"  HTTP {status} for {url}")
            return None

        try:
            return resp.json()
        except Exception as exc:
            print(f"  Could not parse JSON from {url}: {exc!r}")
            return None
    return None


def get_fabric_loader(mc_version):
    url = f"{FABRIC_META_API}/versions/loader/{mc_version}"
    data = api_get(url)
    if not data:
        return "unknown"
    try:
        return data[0].get("loader", {}).get("version") or "unknown"
    except Exception:
        return "unknown"


def fetch_latest_compatible(project_slug, target_mc, loader, allow_base_fallback=False):
    url = f"{MODRINTH_API}/project/{project_slug}/version"

    versions_to_check = [target_mc]
    if allow_base_fallback:
        parts = target_mc.split(".")
        if len(parts) >= 3:
            base = ".".join(parts[:2])
            if base != target_mc:
                versions_to_check.append(base)

    params = {"loaders": json.dumps([loader])}
    versions = api_get(url, params=params)
    if not versions or not isinstance(versions, list):
        return None, None

    candidates = []
    for v in versions:
        supported = v.get("game_versions", [])
        for check_mc in versions_to_check:
            if check_mc in supported:
                candidates.append((v, check_mc))
                break

    if not candidates:
        return None, None

    candidates.sort(key=lambda pair: pair[0].get("date_published", ""), reverse=True)
    best_version, used_mc = candidates[0]
    return best_version, used_mc


def collect_dependencies(version, seen_projects):
    deps = []
    for dep in version.get("dependencies", []):
        if dep.get("dependency_type") != "required":
            continue
        dep_project_id = dep.get("project_id")
        dep_version_id = dep.get("version_id")
        if not dep_project_id or dep_project_id in seen_projects:
            continue
        seen_projects.add(dep_project_id)
        if dep_version_id:
            dep_version = api_get(f"{MODRINTH_API}/version/{dep_version_id}")
            if dep_version:
                deps.append(dep_version)
                deps.extend(collect_dependencies(dep_version, seen_projects))
    return deps


def resolve_all_components(mc_version):
    result = {
        "fabric_loader": get_fabric_loader(mc_version),
        "mods": {},
        "shaders": {},
        "resourcepacks": {},
        "fallbacks": [],
    }

    mod_slugs = list(MOD_SLUGS)
    if version_tuple(mc_version) < version_tuple(CIT_RESEWN_CUTOFF):
        mod_slugs.append("cit-resewn")
    if version_tuple(mc_version) < version_tuple(EATING_ANIMATION_MOD_CUTOFF):
        mod_slugs.append("eating-animation")

    seen = set()

    for slug in mod_slugs:
        v, _ = fetch_latest_compatible(slug, mc_version, "fabric", allow_base_fallback=False)
        if v:
            seen.add(v.get("project_id"))
            files = v.get("files", [])
            if files:
                result["mods"][slug] = {"id": v["id"], "filename": files[0]["filename"]}
            for d in collect_dependencies(v, seen):
                dpid = d.get("project_id")
                dfiles = d.get("files", [])
                if dpid and dfiles:
                    result["mods"][dpid] = {"id": d["id"], "filename": dfiles[0]["filename"]}
        else:
            print(f"  Warning: no exact-compatible Fabric release of {slug} for {mc_version}")
        time.sleep(REQUEST_DELAY)

    for slug in SHADER_SLUGS:
        v, used = fetch_latest_compatible(slug, mc_version, "iris", allow_base_fallback=True)
        if v:
            files = v.get("files", [])
            if files:
                result["shaders"][slug] = {"id": v["id"], "filename": files[0]["filename"]}
            if used != mc_version:
                result["fallbacks"].append(f"shader {slug} (used {used})")
                print(f"  Fallback: shader {slug} resolved via {used} for {mc_version}")
        else:
            print(f"  Warning: no compatible shader release of {slug} for {mc_version}")
        time.sleep(REQUEST_DELAY)

    rp_slugs = list(RESOURCE_PACK_SLUGS)
    if version_tuple(mc_version) >= version_tuple("1.21.4"):
        rp_slugs.append(EATING_ANIMATION_PACK_SLUG)

    for slug in rp_slugs:
        v, used = fetch_latest_compatible(slug, mc_version, "minecraft", allow_base_fallback=True)
        if v:
            files = v.get("files", [])
            if files:
                result["resourcepacks"][slug] = {"id": v["id"], "filename": files[0]["filename"]}
            if used != mc_version:
                result["fallbacks"].append(f"pack {slug} (used {used})")
                print(f"  Fallback: resource pack {slug} resolved via {used} for {mc_version}")
        else:
            print(f"  Warning: no compatible resource pack release of {slug} for {mc_version}")
        time.sleep(REQUEST_DELAY)

    return result


def toml_escape(s):
    return str(s).replace("\\", "\\\\").replace('"', '\\"')


def build_manifest(all_data):
    lines = ["# pack_manifest.toml", "# Auto-generated by generate_manifest.py", ""]
    for mc in SUPPORTED_VERSIONS:
        data = all_data.get(mc)
        if not data:
            continue
        lines.append(f'[versions."{mc}"]')
        lines.append(f'fabric_loader = "{toml_escape(data["fabric_loader"])}"')
        lines.append("")
        for section in ("mods", "shaders", "resourcepacks"):
            lines.append(f'[versions."{mc}".{section}]')
            for slug, info in data[section].items():
                lines.append(f'{slug} = "{toml_escape(info["id"])}"')
                lines.append(f'# filename: {toml_escape(info["filename"])}')
            lines.append("")
    return "\n".join(lines)


def main():
    all_data = {}
    for mc in SUPPORTED_VERSIONS:
        print(f"\nResolving components for Minecraft {mc} ...")
        data = resolve_all_components(mc)
        all_data[mc] = data
        note = f" ({len(data['fallbacks'])} via base-version fallback)" if data['fallbacks'] else ""
        print(f"  ✓ {len(data['mods'])} mods, {len(data['shaders'])} shaders, "
              f"{len(data['resourcepacks'])} resource packs{note}")

    toml_text = build_manifest(all_data)
    with open("pack_manifest.toml", "w", encoding="utf-8") as f:
        f.write(toml_text)
    print(f"\n✓ Manifest written to pack_manifest.toml")


if __name__ == "__main__":
    main()