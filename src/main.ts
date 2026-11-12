import { invoke } from "@tauri-apps/api/core";

const installButton = document.getElementById("install-btn") as HTMLButtonElement;
const versionSelect = document.getElementById("version") as HTMLSelectElement;
const statusDiv = document.getElementById("status") as HTMLDivElement;

installButton.addEventListener("click", async () => {
  const mcVersion = versionSelect.value;

  installButton.disabled = true;
  statusDiv.textContent = `Installing Trailer Pack for Minecraft ${mcVersion}...`;

  try {
    const result = await invoke<any>("install_pack", {
      mcVersion: mcVersion,
    });

    statusDiv.innerHTML = `
      <strong>Done.</strong><br>
      ${result.message}<br>
      <small>Mods: ${result.mods_installed.length},
             shaders: ${result.shaders_installed.length},
             packs: ${result.packs_installed.length},
             profile: ${result.profile_created ? "created" : "not created"}</small>
    `;
  } catch (error) {
    statusDiv.innerHTML = `<strong>Error:</strong> ${error}`;
  } finally {
    installButton.disabled = false;
  }
});