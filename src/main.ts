import { invoke } from "@tauri-apps/api/core";

const installButton = document.getElementById("install-btn") as HTMLButtonElement;
const versionSelect = document.getElementById("version") as HTMLSelectElement;
const launcherSelect = document.getElementById("launcher") as HTMLSelectElement;
const statusDiv = document.getElementById("status") as HTMLDivElement;

installButton.addEventListener("click", async () => {
  const mcVersion = versionSelect.value;
  const launcherType = launcherSelect.value;

  installButton.disabled = true;
  statusDiv.textContent = `Installing Trailer Pack for Minecraft ${mcVersion} (${launcherType})...`;

  try {
    const result = await invoke<any>("install_pack", {
      mcVersion: mcVersion,
      launcherType: launcherType,
    });

    statusDiv.innerHTML = `
      <strong>Done.</strong><br>
      ${result.message}<br>
      <small>Mods: ${result.mods_installed.length},
             shaders: ${result.shaders_installed.length},
             packs: ${result.packs_installed.length}</small>
    `;
  } catch (error) {
    statusDiv.innerHTML = `<strong>Error:</strong> ${error}`;
  } finally {
    installButton.disabled = false;
  }
});