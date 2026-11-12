use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::Manager;

#[derive(Deserialize, Debug)]
struct Manifest {
    versions: HashMap<String, VersionEntry>,
}

#[derive(Deserialize, Debug)]
struct VersionEntry {
    fabric_loader: String,
    #[serde(default)]
    mods: HashMap<String, String>,
    #[serde(default)]
    shaders: HashMap<String, String>,
    #[serde(default)]
    resourcepacks: HashMap<String, String>,
}

#[derive(Deserialize)]
struct ModrinthVersion {
    files: Vec<ModrinthFile>,
}

#[derive(Deserialize)]
struct ModrinthFile {
    url: String,
    filename: String,
}

#[derive(Serialize)]
struct InstallResult {
    mc_version: String,
    mods_installed: Vec<String>,
    shaders_installed: Vec<String>,
    packs_installed: Vec<String>,
    iris_config_written: bool,
    options_written: bool,
    profile_created: bool,
    profile_status: String,
    fabric_status: String,
    message: String,
}

fn resolve_manifest_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(p) = app
        .path()
        .resolve("pack_manifest.toml", tauri::path::BaseDirectory::Resource)
    {
        if p.exists() {
            return Ok(p);
        }
    }

    let candidates = [
        PathBuf::from("src-tauri/pack_manifest.toml"),
        PathBuf::from("../src-tauri/pack_manifest.toml"),
        PathBuf::from("pack_manifest.toml"),
        PathBuf::from("../pack_manifest.toml"),
    ];
    for c in candidates {
        if c.exists() {
            return Ok(c.canonicalize().unwrap_or(c));
        }
    }

    Err(
        "Could not find pack_manifest.toml. Make sure the file exists at \
         src-tauri/pack_manifest.toml and that tauri.conf.json lists it under \
         bundle.resources."
            .to_string(),
    )
}

fn detect_minecraft_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| "APPDATA environment variable is not set".to_string())?;
    let path = PathBuf::from(appdata).join(".minecraft");
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "Minecraft folder not found at {} — is Minecraft Launcher installed?",
            path.display()
        ))
    }
}

async fn fetch_pinned_file(
    client: &Client,
    slug: &str,
    version_id: &str,
) -> Result<ModrinthFile, String> {
    let url = format!(
        "https://api.modrinth.com/v2/project/{}/version/{}",
        slug, version_id
    );
    let resp = client
        .get(&url)
        .header("User-Agent", "TrailerPackInstaller/1.0")
        .send()
        .await
        .map_err(|e| format!("Network error fetching {}: {}", slug, e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Modrinth returned {} for {} / {}",
            resp.status(),
            slug,
            version_id
        ));
    }

    let version: ModrinthVersion = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Modrinth response for {}: {}", slug, e))?;

    version
        .files
        .into_iter()
        .next()
        .ok_or_else(|| format!("No files in Modrinth response for {}", slug))
}

async fn download_file(client: &Client, url: &str, dest: &Path) -> Result<(), String> {
    let resp = client
        .get(url)
        .header("User-Agent", "TrailerPackInstaller/1.0")
        .send()
        .await
        .map_err(|e| format!("Download error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download HTTP error: {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response bytes: {}", e))?;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    fs::write(dest, &bytes)
        .map_err(|e| format!("Failed to write {}: {}", dest.display(), e))?;
    Ok(())
}

fn write_iris_config(instance_dir: &Path, shader_filename: &str) -> Result<(), String> {
    let config_dir = instance_dir.join("config");
    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let iris_path = config_dir.join("iris.properties");

    let mut lines: Vec<String> = if iris_path.exists() {
        fs::read_to_string(&iris_path)
            .map_err(|e| e.to_string())?
            .lines()
            .map(String::from)
            .collect()
    } else {
        Vec::new()
    };

    let mut has_enable = false;
    let mut has_shader = false;

    for line in &mut lines {
        if line.starts_with("enableShaders=") {
            *line = "enableShaders=true".to_string();
            has_enable = true;
        } else if line.starts_with("shaderPack=") {
            *line = format!("shaderPack={}", shader_filename);
            has_shader = true;
        }
    }
    if !has_enable {
        lines.push("enableShaders=true".to_string());
    }
    if !has_shader {
        lines.push(format!("shaderPack={}", shader_filename));
    }

    fs::write(&iris_path, lines.join("\n")).map_err(|e| e.to_string())?;

    let content = fs::read_to_string(&iris_path).map_err(|e| e.to_string())?;
    if !content.contains("enableShaders=true") {
        return Err(
            "Verification failed: 'enableShaders=true' not present in iris.properties".into(),
        );
    }
    let expected = format!("shaderPack={}", shader_filename);
    if !content.contains(&expected) {
        return Err(format!(
            "Verification failed: '{}' not present in iris.properties",
            expected
        ));
    }

    Ok(())
}

fn update_resource_packs(
    instance_dir: &Path,
    ordered_pack_filenames: &[String],
) -> Result<(), String> {
    let opts_path = instance_dir.join("options.txt");
    let content = if opts_path.exists() {
        fs::read_to_string(&opts_path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };

    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let mut existing_packs: Vec<String> = Vec::new();

    for line in &lines {
        if line.starts_with("resourcePacks:") {
            let raw = line.trim_start_matches("resourcePacks:");
            if let Ok(arr) = serde_json::from_str::<Vec<String>>(raw) {
                existing_packs = arr;
            }
            break;
        }
    }

    let tp_set: HashSet<&String> = ordered_pack_filenames.iter().collect();
    let mut merged: Vec<String> = existing_packs
        .into_iter()
        .filter(|p| !tp_set.contains(p))
        .collect();

    for pack in ordered_pack_filenames {
        if !merged.contains(pack) {
            merged.push(pack.clone());
        }
    }

    let new_line = format!(
        "resourcePacks:{}",
        serde_json::to_string(&merged).map_err(|e| e.to_string())?
    );

    let mut replaced = false;
    for line in &mut lines {
        if line.starts_with("resourcePacks:") {
            *line = new_line.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        lines.push(new_line);
    }

    let mut has_incompat = false;
    for line in &mut lines {
        if line.starts_with("incompatibleResourcePacks:") {
            *line = "incompatibleResourcePacks:[]".to_string();
            has_incompat = true;
            break;
        }
    }
    if !has_incompat {
        lines.push("incompatibleResourcePacks:[]".to_string());
    }

    fs::write(&opts_path, lines.join("\n")).map_err(|e| e.to_string())?;

    let verify = fs::read_to_string(&opts_path).map_err(|e| e.to_string())?;
    for pack in ordered_pack_filenames {
        if !verify.contains(pack) {
            return Err(format!(
                "Verification failed: pack '{}' not found in options.txt",
                pack
            ));
        }
    }
    Ok(())
}

fn create_launcher_profile(
    minecraft_dir: &Path,
    mc_version: &str,
    fabric_loader: &str,
    game_dir: &Path,
) -> Result<String, String> {
    if fabric_loader == "unknown" || fabric_loader.is_empty() {
        return Err(
            "Manifest has no Fabric loader version for this Minecraft version — cannot create launcher profile."
                .into(),
        );
    }

    let profile_path = minecraft_dir.join("launcher_profiles.json");
    if !profile_path.exists() {
        return Err(
            "launcher_profiles.json not found. Launch Minecraft at least once so the launcher creates it, then re-run the installer."
                .into(),
        );
    }

    let text = fs::read_to_string(&profile_path)
        .map_err(|e| format!("Failed to read launcher_profiles.json (is the launcher open?): {}", e))?;
    let mut root: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Failed to parse launcher_profiles.json: {}", e))?;

    let version_id = format!("fabric-loader-{}-{}", fabric_loader, mc_version);

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let profile = serde_json::json!({
        "name": "Trailer Pack",
        "type": "custom",
        "created": now,
        "lastUsed": now,
        "lastVersionId": version_id,
        "javaArgs": "-Xmx4G -XX:+UseG1GC -XX:+ParallelRefProcEnabled",
        "icon": "Furnace",
        "logConfig": null,
        "logConfigIsXML": false,
        "gameDir": game_dir.canonicalize().unwrap_or_else(|_| game_dir.to_path_buf())
    });

    if root.get("profiles").is_none() {
        root["profiles"] = serde_json::json!({});
    }
    root["profiles"]["Trailer Pack"] = profile;

    let serialized = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize launcher_profiles.json: {}", e))?;

    fs::write(&profile_path, serialized)
        .map_err(|e| format!("Failed to write launcher_profiles.json (is the launcher open?): {}", e))?;

    Ok(format!(
        "Launcher profile 'Trailer Pack' created, using game directory {}.",
        game_dir.display()
    ))
}

fn find_java(minecraft_dir: &Path) -> Option<PathBuf> {
    if let Ok(java) = which::which("java") {
        return Some(java);
    }

    let runtime_dir = minecraft_dir.join("runtime");
    if runtime_dir.exists() {
        let candidates = [
            runtime_dir.join("java-runtime-gamma").join("bin").join("java.exe"),
            runtime_dir.join("java-runtime-delta").join("bin").join("java.exe"),
            runtime_dir.join("jre-legacy").join("bin").join("java.exe"),
        ];
        for c in candidates {
            if c.exists() {
                return Some(c);
            }
        }
        if let Ok(entries) = fs::read_dir(&runtime_dir) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("bin").join("java.exe");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

async fn download_fabric_installer(client: &Client) -> Result<PathBuf, String> {
    let version = "1.1.2";
    let url = format!(
        "https://maven.fabricmc.net/net/fabricmc/fabric-installer/{}/fabric-installer-{}.jar",
        version, version
    );
    let temp_dir = std::env::temp_dir().join("trailer-pack-installer");
    fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    let dest = temp_dir.join(format!("fabric-installer-{}.jar", version));

    let resp = client
        .get(&url)
        .header("User-Agent", "TrailerPackInstaller/1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to download Fabric installer: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Fabric installer download returned HTTP {}",
            resp.status()
        ));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    Ok(dest)
}

async fn install_fabric(
    client: &Client,
    minecraft_dir: &Path,
    mc_version: &str,
    loader_version: &str,
) -> Result<String, String> {
    let version_id = format!("fabric-loader-{}-{}", loader_version, mc_version);
    let version_dir = minecraft_dir.join("versions").join(&version_id);
    if version_dir.exists() {
        return Ok(format!("Fabric {} already installed.", version_id));
    }

    let java = find_java(minecraft_dir).ok_or_else(|| {
        "Java not found. Install Java (https://adoptium.net/) or run Minecraft Launcher at least once so its bundled Java is available.".to_string()
    })?;

    let installer = download_fabric_installer(client).await?;

    let status = std::process::Command::new(&java)
        .arg("-jar")
        .arg(&installer)
        .arg("client")
        .arg("-mcversion")
        .arg(mc_version)
        .arg("-loader")
        .arg(loader_version)
        .arg("-noprofile")
        .arg("-dir")
        .arg(minecraft_dir)
        .output()
        .map_err(|e| format!("Failed to run Fabric installer: {}", e))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        let stdout = String::from_utf8_lossy(&status.stdout);
        return Err(format!(
            "Fabric installer failed (exit {}).\nstdout: {}\nstderr: {}",
            status.status.code().unwrap_or(-1),
            stdout.trim(),
            stderr.trim()
        ));
    }

    if !version_dir.exists() {
        return Err(format!(
            "Fabric installer finished but {} was not created. The launcher may have blocked file writes — close Minecraft Launcher and retry.",
            version_id
        ));
    }

    Ok(format!("Fabric {} installed successfully.", version_id))
}

const PACK_ORDER: &[&str] = &[
    "bare-bones",
    "bare-bones-better-leaves",
    "bare-bones-connected-glass",
    "bare-bones-low-on-fire",
    "better-lanterns",
    "cubic-sun-moon",
    "even-better-enchants",
    "low-shield-pack",
    "new-glowing-ores",
    "visual-armor-trims",
    "fresh-animations",
    "bare-bones-x-eating-animation",
    "theones-eating-animation-pack",
    "bare-bones-x-fresh-animations",
];

#[tauri::command]
async fn install_pack(
    app: tauri::AppHandle,
    mc_version: String,
    minecraft_dir: Option<String>,
) -> Result<InstallResult, String> {
    let manifest_path = resolve_manifest_path(&app)?;
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read manifest at {}: {}", manifest_path.display(), e))?;
    let manifest: Manifest =
        toml::from_str(&manifest_text).map_err(|e| format!("Failed to parse manifest: {}", e))?;

    let entry = manifest
        .versions
        .get(&mc_version)
        .ok_or_else(|| format!("Minecraft version '{}' is not in the manifest", mc_version))?;

    let mc_dir = match minecraft_dir {
        Some(p) => PathBuf::from(p),
        None => detect_minecraft_dir()?,
    };
    if !mc_dir.exists() {
        return Err(format!(
            "Minecraft folder does not exist: {}",
            mc_dir.display()
        ));
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let fabric_status = install_fabric(&client, &mc_dir, &mc_version, &entry.fabric_loader)
        .await
        .unwrap_or_else(|e| format!("Fabric installation failed: {}", e));

    let version_id = format!("fabric-loader-{}-{}", entry.fabric_loader, mc_version);
    let instance_dir = mc_dir.join("versions").join(&version_id);
    fs::create_dir_all(&instance_dir).map_err(|e| {
        format!(
            "Failed to create instance directory {}: {}",
            instance_dir.display(),
            e
        )
    })?;

    let mods_dir = instance_dir.join("mods");
    let shaders_dir = instance_dir.join("shaderpacks");
    let packs_dir = instance_dir.join("resourcepacks");

    let mut mods_installed = Vec::new();
    for (slug, version_id) in &entry.mods {
        let file = fetch_pinned_file(&client, slug, version_id).await?;
        let dest = mods_dir.join(&file.filename);
        download_file(&client, &file.url, &dest).await?;
        mods_installed.push(file.filename);
    }

    let mut shader_filename = String::new();
    let mut shaders_installed = Vec::new();
    for (slug, version_id) in &entry.shaders {
        let file = fetch_pinned_file(&client, slug, version_id).await?;
        let dest = shaders_dir.join(&file.filename);
        download_file(&client, &file.url, &dest).await?;
        shader_filename = file.filename.clone();
        shaders_installed.push(file.filename);
    }

    let mut pack_filenames: HashMap<String, String> = HashMap::new();
    let mut packs_installed = Vec::new();
    for (slug, version_id) in &entry.resourcepacks {
        let file = fetch_pinned_file(&client, slug, version_id).await?;
        let dest = packs_dir.join(&file.filename);
        download_file(&client, &file.url, &dest).await?;
        pack_filenames.insert(slug.clone(), file.filename.clone());
        packs_installed.push(file.filename);
    }

    let mut ordered_pack_filenames = Vec::new();
    for slug in PACK_ORDER {
        if let Some(fname) = pack_filenames.get(*slug) {
            ordered_pack_filenames.push(fname.clone());
        }
    }

    if shader_filename.is_empty() {
        return Err("No shader was downloaded — cannot configure Iris".into());
    }
    write_iris_config(&instance_dir, &shader_filename)?;

    update_resource_packs(&instance_dir, &ordered_pack_filenames)?;

    let profile_status = create_launcher_profile(
        &mc_dir,
        &mc_version,
        &entry.fabric_loader,
        &instance_dir,
    )
    .unwrap_or_else(|e| format!("Profile creation failed: {}", e));
    let profile_created = !profile_status.starts_with("Profile creation failed");

    let summary = format!(
        "Trailer Pack installed for Minecraft {}: {} mods, {} shader(s), {} resource packs. \
         iris.properties and options.txt written to {}. {} {}",
        mc_version,
        mods_installed.len(),
        shaders_installed.len(),
        packs_installed.len(),
        instance_dir.display(),
        fabric_status,
        profile_status
    );

    Ok(InstallResult {
        mc_version,
        mods_installed,
        shaders_installed,
        packs_installed,
        iris_config_written: true,
        options_written: true,
        profile_created,
        profile_status,
        fabric_status,
        message: summary,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![install_pack])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}