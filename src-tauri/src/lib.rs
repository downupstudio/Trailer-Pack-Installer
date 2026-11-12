use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tauri::Manager;
use zip::ZipArchive;

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
    if let Ok(p) = app.path().resolve("pack_manifest.toml", tauri::path::BaseDirectory::Resource) {
        if p.exists() { return Ok(p); }
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let p1 = exe_dir.join("pack_manifest.toml");
            if p1.exists() { return Ok(p1); }
            let p2 = exe_dir.join("resources").join("pack_manifest.toml");
            if p2.exists() { return Ok(p2); }
        }
    }
    let candidates = [
        PathBuf::from("src-tauri/pack_manifest.toml"),
        PathBuf::from("../src-tauri/pack_manifest.toml"),
        PathBuf::from("pack_manifest.toml"),
    ];
    for c in candidates {
        if c.exists() { return Ok(c.canonicalize().unwrap_or(c)); }
    }
    Err("Could not find pack_manifest.toml.".to_string())
}

fn detect_minecraft_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA not set".to_string())?;
    let path = PathBuf::from(appdata).join(".minecraft");
    if path.exists() { Ok(path) } else { Err("Minecraft folder not found.".to_string()) }
}

async fn fetch_pinned_file(client: &Client, slug: &str, version_id: &str) -> Result<ModrinthFile, String> {
    let url = format!("https://api.modrinth.com/v2/project/{}/version/{}", slug, version_id);
    let resp = client.get(&url).header("User-Agent", "TrailerPackInstaller/1.0").send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() { return Err(format!("Modrinth error {}", resp.status())); }
    let version: ModrinthVersion = resp.json().await.map_err(|e| e.to_string())?;
    version.files.into_iter().next().ok_or_else(|| "No files in Modrinth response".to_string())
}

async fn download_file(client: &Client, url: &str, dest: &Path) -> Result<(), String> {
    let resp = client.get(url).header("User-Agent", "TrailerPackInstaller/1.0").send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() { return Err(format!("HTTP error {}", resp.status())); }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if let Some(parent) = dest.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::write(dest, &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_iris_config(instance_dir: &Path, shader_filename: &str) -> Result<(), String> {
    let config_dir = instance_dir.join("config");
    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let iris_path = config_dir.join("iris.properties");
    let mut lines: Vec<String> = if iris_path.exists() {
        fs::read_to_string(&iris_path).map_err(|e| e.to_string())?.lines().map(String::from).collect()
    } else { Vec::new() };
    let mut has_enable = false;
    let mut has_shader = false;
    for line in &mut lines {
        if line.starts_with("enableShaders=") { *line = "enableShaders=true".to_string(); has_enable = true; }
        else if line.starts_with("shaderPack=") { *line = format!("shaderPack={}", shader_filename); has_shader = true; }
    }
    if !has_enable { lines.push("enableShaders=true".to_string()); }
    if !has_shader { lines.push(format!("shaderPack={}", shader_filename)); }
    fs::write(&iris_path, lines.join("\n")).map_err(|e| e.to_string())?;
    let content = fs::read_to_string(&iris_path).map_err(|e| e.to_string())?;
    if !content.contains("enableShaders=true") || !content.contains(&format!("shaderPack={}", shader_filename)) {
        return Err("Iris config verification failed".into());
    }
    Ok(())
}

fn update_resource_packs(instance_dir: &Path, ordered_pack_filenames: &[String]) -> Result<(), String> {
    let opts_path = instance_dir.join("options.txt");
    let content = if opts_path.exists() { fs::read_to_string(&opts_path).map_err(|e| e.to_string())? } else { String::new() };
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let mut existing_packs: Vec<String> = Vec::new();
    for line in &lines {
        if line.starts_with("resourcePacks:") {
            if let Ok(arr) = serde_json::from_str::<Vec<String>>(line.trim_start_matches("resourcePacks:")) { existing_packs = arr; }
            break;
        }
    }
    let tp_set: HashSet<&String> = ordered_pack_filenames.iter().collect();
    let mut merged: Vec<String> = existing_packs.into_iter().filter(|p| !tp_set.contains(p)).collect();
    for pack in ordered_pack_filenames { if !merged.contains(pack) { merged.push(pack.clone()); } }
    let new_line = format!("resourcePacks:{}", serde_json::to_string(&merged).map_err(|e| e.to_string())?);
    let mut replaced = false;
    for line in &mut lines { if line.starts_with("resourcePacks:") { *line = new_line.clone(); replaced = true; break; } }
    if !replaced { lines.push(new_line); }
    let mut has_incompat = false;
    for line in &mut lines { if line.starts_with("incompatibleResourcePacks:") { *line = "incompatibleResourcePacks:[]".to_string(); has_incompat = true; break; } }
    if !has_incompat { lines.push("incompatibleResourcePacks:[]".to_string()); }
    fs::write(&opts_path, lines.join("\n")).map_err(|e| e.to_string())?;
    Ok(())
}

fn create_launcher_profile(minecraft_dir: &Path, mc_version: &str, fabric_loader: &str, game_dir: &Path) -> Result<String, String> {
    let profile_path = minecraft_dir.join("launcher_profiles.json");
    if !profile_path.exists() { return Err("launcher_profiles.json not found. Run MC once.".into()); }
    let text = fs::read_to_string(&profile_path).map_err(|e| e.to_string())?;
    let mut root: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let version_id = format!("fabric-loader-{}-{}", fabric_loader, mc_version);
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let profile = serde_json::json!({
        "name": "Trailer Pack", "type": "custom", "created": now, "lastUsed": now,
        "lastVersionId": version_id, "javaArgs": "-Xmx4G", "icon": "Furnace",
        "gameDir": game_dir.canonicalize().unwrap_or_else(|_| game_dir.to_path_buf())
    });
    if root.get("profiles").is_none() { root["profiles"] = serde_json::json!({}); }
    root["profiles"]["Trailer Pack"] = profile;
    fs::write(&profile_path, serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok("Profile created.".to_string())
}

fn find_java(minecraft_dir: &Path) -> Option<PathBuf> {
    if let Ok(java) = which::which("java") { return Some(java); }
    let runtime_dir = minecraft_dir.join("runtime");
    if runtime_dir.exists() {
        if let Ok(entries) = fs::read_dir(&runtime_dir) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("bin").join("java.exe");
                if candidate.exists() { return Some(candidate); }
            }
        }
    }
    None
}

async fn download_fabric_installer(client: &Client) -> Result<PathBuf, String> {
    let version = "1.1.2";
    let url = format!("https://maven.fabricmc.net/net/fabricmc/fabric-installer/{}/fabric-installer-{}.jar", version, version);
    let temp_dir = std::env::temp_dir().join("trailer-pack-installer");
    fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    let dest = temp_dir.join(format!("fabric-installer-{}.jar", version));
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() { return Err("Fabric installer download failed".to_string()); }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    Ok(dest)
}

async fn install_fabric(client: &Client, target_dir: &Path, mc_version: &str, loader_version: &str) -> Result<String, String> {
    let version_id = format!("fabric-loader-{}-{}", loader_version, mc_version);
    if target_dir.join("versions").join(&version_id).exists() { return Ok("Fabric already installed.".to_string()); }
    let java = find_java(target_dir).ok_or_else(|| "Java not found.".to_string())?;
    let installer = download_fabric_installer(client).await?;
    let status = std::process::Command::new(&java).arg("-jar").arg(&installer).arg("client")
        .arg("-mcversion").arg(mc_version).arg("-loader").arg(loader_version)
        .arg("-noprofile").arg("-dir").arg(target_dir).output().map_err(|e| e.to_string())?;
    if !status.status.success() { return Err("Fabric installation failed".to_string()); }
    Ok("Fabric installed.".to_string())
}

async fn download_and_extract_natives(client: &Client, target_dir: &Path) -> Result<(), String> {
    let nt_static = target_dir.join("nt_static");
    let lwjgl_path = nt_static.join("windows").join("x64").join("org").join("lwjgl");
    fs::create_dir_all(&lwjgl_path).map_err(|e| e.to_string())?;

    let jars = [
        ("https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar", "lwjgl"),
        ("https://libraries.minecraft.net/org/lwjgl/lwjgl-glfw/3.3.3/lwjgl-glfw-3.3.3-natives-windows.jar", "glfw"),
        ("https://libraries.minecraft.net/org/lwjgl/lwjgl-openal/3.3.3/lwjgl-openal-3.3.3-natives-windows.jar", "openal"),
        ("https://libraries.minecraft.net/org/lwjgl/lwjgl-opengl/3.3.3/lwjgl-opengl-3.3.3-natives-windows.jar", "opengl"),
        ("https://libraries.minecraft.net/org/lwjgl/lwjgl-stb/3.3.3/lwjgl-stb-3.3.3-natives-windows.jar", "stb"),
        ("https://libraries.minecraft.net/org/lwjgl/lwjgl-tinyfd/3.3.3/lwjgl-tinyfd-3.3.3-natives-windows.jar", "tinyfd"),
    ];

    for (url, folder) in jars {
        let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() { continue; }
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        let cursor = Cursor::new(bytes);
        let mut archive = ZipArchive::new(cursor).map_err(|e| e.to_string())?;
        
        let dest_folder = lwjgl_path.join(folder);
        fs::create_dir_all(&dest_folder).map_err(|e| e.to_string())?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.name().ends_with(".dll") {
                let outpath = dest_folder.join(Path::new(file.name()).file_name().unwrap());
                let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
                std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
            }
        }
    }

    let meta_inf = nt_static.join("META-INF");
    fs::create_dir_all(&meta_inf).map_err(|e| e.to_string())?;
    fs::write(meta_inf.join("MANIFEST.MF"), "Manifest-Version: 1.0\r\nCreated-By: Trailer Pack Installer\r\n\r\n").map_err(|e| e.to_string())?;
    fs::write(meta_inf.join("INDEX.LIST"), "").map_err(|e| e.to_string())?;

    let version_dir = meta_inf.join("versions").join("9");
    fs::create_dir_all(&version_dir).map_err(|e| e.to_string())?;
    
    let minimal_class: &[u8] = &[
        0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 0x37, 0x00, 0x0A, 0x07, 0x00, 0x02, 0x01, 0x00, 0x06,
        0x6D, 0x6F, 0x64, 0x75, 0x6C, 0x65, 0x07, 0x00, 0x04, 0x01, 0x00, 0x06, 0x45, 0x78, 0x70, 0x6F,
        0x72, 0x74, 0x73, 0x00, 0x00
    ];
    fs::write(version_dir.join("module-info.class"), minimal_class).map_err(|e| e.to_string())?;

    let cached_info = nt_static.join("__cachedInfo.dat");
    if !cached_info.exists() {
        fs::write(cached_info, "").map_err(|e| e.to_string())?;
    }

    Ok(())
}

const PACK_ORDER: &[&str] = &[
    "bare-bones", "bare-bones-better-leaves", "bare-bones-connected-glass", "bare-bones-low-on-fire",
    "better-lanterns", "cubic-sun-moon", "even-better-enchants", "low-shield-pack",
    "new-glowing-ores", "visual-armor-trims", "fresh-animations", "bare-bones-x-eating-animation",
    "theones-eating-animation-pack", "bare-bones-x-fresh-animations",
];

#[tauri::command]
async fn install_pack(app: tauri::AppHandle, mc_version: String, launcher_type: String, minecraft_dir: Option<String>) -> Result<InstallResult, String> {
    let manifest_path = resolve_manifest_path(&app)?;
    let manifest_text = fs::read_to_string(&manifest_path).map_err(|e| format!("Failed to read manifest: {}", e))?;
    let manifest: Manifest = toml::from_str(&manifest_text).map_err(|e| format!("Failed to parse manifest: {}", e))?;
    let entry = manifest.versions.get(&mc_version).ok_or_else(|| format!("Version {} not in manifest", mc_version))?;

    let client = Client::builder().timeout(std::time::Duration::from_secs(120)).build().map_err(|e| e.to_string())?;
    let is_crystal = launcher_type == "crystal";

    let (root_mc_dir, instance_dir) = if is_crystal {
        let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA not set".to_string())?;
        let instances_dir = PathBuf::from(appdata).join("Crystal-Launcher").join("instances");
        
        if !instances_dir.exists() {
            return Err("Crystal Launcher not found. Please run it once.".to_string());
        }
        
        let pack_display_name = format!("Trailer Pack {}", mc_version);
        let instance_folder_name = format!("u.{}", pack_display_name);
        let crystal_instance = instances_dir.join(&instance_folder_name);
        let dot_mc = crystal_instance.join(".minecraft");
        
        fs::create_dir_all(&dot_mc).map_err(|e| e.to_string())?;

        fs::write(crystal_instance.join("packName.flag"), &pack_display_name).map_err(|e| e.to_string())?;
        fs::write(crystal_instance.join(".fbcmlversion"), format!("{}|{}", mc_version, entry.fabric_loader)).map_err(|e| e.to_string())?;
        fs::write(crystal_instance.join("FabricMcInstalled"), format!("fabric-loader-{}-{}", entry.fabric_loader, mc_version)).map_err(|e| e.to_string())?;

        let profile_path = dot_mc.join("launcher_profiles.json");
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+0100").to_string();
        let game_dir_str = dot_mc.to_string_lossy().replace("\\", "\\\\");
        
        let profiles_json = serde_json::json!({
            "profiles": {
                mc_version.clone(): {
                    "name": mc_version,
                    "lastVersionId": mc_version,
                    "gameDir": game_dir_str
                },
                format!("fabric-loader-{}", mc_version): {
                    "name": format!("fabric-loader-{}", mc_version),
                    "type": "custom",
                    "created": now,
                    "lastUsed": now,
                    "icon": "TNT",
                    "lastVersionId": format!("fabric-loader-{}-{}", entry.fabric_loader, mc_version)
                }
            },
            "selectedProfile": mc_version
        });
        fs::write(profile_path, serde_json::to_string_pretty(&profiles_json).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;

        download_and_extract_natives(&client, &crystal_instance).await?;
        
        (dot_mc.clone(), dot_mc)
    } else {
        let mc_dir = match minecraft_dir { Some(p) => PathBuf::from(p), None => detect_minecraft_dir()? };
        let version_id = format!("fabric-loader-{}-{}", entry.fabric_loader, mc_version);
        let inst_dir = mc_dir.join("versions").join(&version_id);
        fs::create_dir_all(&inst_dir).map_err(|e| e.to_string())?;
        (mc_dir, inst_dir)
    };

    let fabric_status = install_fabric(&client, &root_mc_dir, &mc_version, &entry.fabric_loader).await.unwrap_or_else(|e| format!("Error: {}", e));

    let mods_dir = instance_dir.join("mods");
    let shaders_dir = instance_dir.join("shaderpacks");
    let packs_dir = instance_dir.join("resourcepacks");

    let mut mods_installed = Vec::new();
    for (slug, version_id) in &entry.mods {
        let file = fetch_pinned_file(&client, slug, version_id).await?;
        download_file(&client, &file.url, &mods_dir.join(&file.filename)).await?;
        mods_installed.push(file.filename);
    }

    let mut shader_filename = String::new();
    let mut shaders_installed = Vec::new();
    for (slug, version_id) in &entry.shaders {
        let file = fetch_pinned_file(&client, slug, version_id).await?;
        download_file(&client, &file.url, &shaders_dir.join(&file.filename)).await?;
        shader_filename = file.filename.clone();
        shaders_installed.push(file.filename);
    }

    let mut pack_filenames = HashMap::new();
    let mut packs_installed = Vec::new();
    for (slug, version_id) in &entry.resourcepacks {
        let file = fetch_pinned_file(&client, slug, version_id).await?;
        download_file(&client, &file.url, &packs_dir.join(&file.filename)).await?;
        pack_filenames.insert(slug.clone(), file.filename.clone());
        packs_installed.push(file.filename);
    }

    let mut ordered_pack_filenames = Vec::new();
    for slug in PACK_ORDER { if let Some(fname) = pack_filenames.get(*slug) { ordered_pack_filenames.push(fname.clone()); } }

    if shader_filename.is_empty() { return Err("No shader downloaded.".into()); }
    write_iris_config(&instance_dir, &shader_filename)?;
    update_resource_packs(&instance_dir, &ordered_pack_filenames)?;

    let profile_status = if is_crystal {
        format!("Instance created in Crystal Launcher.")
    } else {
        create_launcher_profile(&root_mc_dir, &mc_version, &entry.fabric_loader, &instance_dir).unwrap_or_else(|e| format!("Profile error: {}", e))
    };

    let summary = format!("Installed {} mods, {} shaders, {} packs. {}", mods_installed.len(), shaders_installed.len(), packs_installed.len(), profile_status);

    Ok(InstallResult {
        mc_version, mods_installed, shaders_installed, packs_installed,
        iris_config_written: true, options_written: true,
        profile_created: !profile_status.contains("error"), profile_status, fabric_status, message: summary,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default().invoke_handler(tauri::generate_handler![install_pack]).run(tauri::generate_context!()).expect("error");
}