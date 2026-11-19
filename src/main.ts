import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

type Mode = "standard" | "assist";

type AssistConfig = {
  provider: string;
  apiKey?: string | null;
  endpoint?: string | null;
  model?: string | null;
};

type RunLogLine = { level: string; message: string };

type RunResult = {
  ok: boolean;
  filesWritten: string[];
  log: RunLogLine[];
  assist: {
    mode: string;
    provider: string;
    configured: boolean;
    liveCallsEnabled: boolean;
    message: string;
  };
};

const state = {
  mode: "standard" as Mode,
  inputPaths: [] as string[],
};

const $ = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;

function appendLog(level: string, message: string) {
  const log = $("log");
  const line = document.createElement("div");
  line.className = level;
  const ts = new Date().toLocaleTimeString();
  line.textContent = `[${ts}] ${level.toUpperCase()}  ${message}`;
  log.appendChild(line);
  log.scrollTop = log.scrollHeight;
}

function setMode(mode: Mode) {
  state.mode = mode;
  $("mode-standard").classList.toggle("is-active", mode === "standard");
  $("mode-assist").classList.toggle("is-active", mode === "assist");
  $("assist-panel").classList.toggle("is-hidden", mode !== "assist");
  refreshAssistStatus();
}

function readAssistConfig(): AssistConfig {
  return {
    provider: ($("assist-provider") as HTMLSelectElement).value,
    apiKey: ($("assist-api-key") as HTMLInputElement).value || null,
    endpoint: ($("assist-endpoint") as HTMLInputElement).value || null,
    model: ($("assist-model") as HTMLInputElement).value || null,
  };
}

async function refreshAssistStatus() {
  try {
    const status = await invoke<{ message: string }>("get_assist_status", {
      mode: state.mode,
      assist: readAssistConfig(),
    });
    $("assist-status").textContent = status.message;
  } catch (err) {
    $("assist-status").textContent = String(err);
  }
}

function selectedIsoStandards(): string[] {
  const boxes = document.querySelectorAll<HTMLInputElement>(
    "#iso-standards input[type=checkbox]",
  );
  return Array.from(boxes)
    .filter((b) => b.checked)
    .map((b) => b.value);
}

function syncInputPathsDisplay() {
  const el = $("input-paths") as HTMLInputElement;
  el.value = state.inputPaths.join("; ");
}

async function pickTemplate() {
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "Word", extensions: ["docx"] }],
  });
  if (typeof selected === "string") {
    ($("template-path") as HTMLInputElement).value = selected;
  }
}

async function pickFiles() {
  const selected = await open({
    multiple: true,
    directory: false,
    filters: [{ name: "Word", extensions: ["docx"] }],
  });
  if (Array.isArray(selected)) {
    state.inputPaths = selected;
  } else if (typeof selected === "string") {
    state.inputPaths = [selected];
  }
  syncInputPathsDisplay();
}

async function pickFolder() {
  const selected = await open({
    multiple: false,
    directory: true,
  });
  if (typeof selected === "string") {
    state.inputPaths = [selected];
    syncInputPathsDisplay();
  }
}

async function pickOutput() {
  const selected = await open({
    multiple: false,
    directory: true,
  });
  if (typeof selected === "string") {
    ($("output-dir") as HTMLInputElement).value = selected;
  }
}

async function runPipeline() {
  const btn = $("run-btn") as HTMLButtonElement;
  btn.disabled = true;
  try {
    const request = {
      mode: state.mode,
      inputPaths: state.inputPaths,
      outputDir: ($("output-dir") as HTMLInputElement).value,
      iso: {
        enabled: ($("iso-enabled") as HTMLInputElement).checked,
        standards: selectedIsoStandards(),
      },
      template: {
        enabled: ($("template-enabled") as HTMLInputElement).checked,
        templatePath: ($("template-path") as HTMLInputElement).value || null,
      },
      context: {
        enabled: ($("context-enabled") as HTMLInputElement).checked,
        text: ($("context-text") as HTMLTextAreaElement).value,
      },
      assist: state.mode === "assist" ? readAssistConfig() : null,
    };

    if (!request.outputDir) {
      appendLog("error", "Choose an output directory.");
      return;
    }
    if (request.inputPaths.length === 0) {
      appendLog("error", "Choose input files or a folder.");
      return;
    }

    appendLog("info", "Starting pipeline…");
    const result = await invoke<RunResult>("run_pipeline", { request });
    for (const line of result.log) {
      appendLog(line.level, line.message);
    }
    appendLog(
      result.ok ? "info" : "error",
      result.ok
        ? `Done. ${result.filesWritten.length} file(s) written.`
        : "Pipeline finished with errors.",
    );
  } catch (err) {
    appendLog("error", String(err));
  } finally {
    btn.disabled = false;
  }
}

window.addEventListener("DOMContentLoaded", () => {
  $("mode-standard").addEventListener("click", () => setMode("standard"));
  $("mode-assist").addEventListener("click", () => setMode("assist"));
  $("pick-template").addEventListener("click", () => void pickTemplate());
  $("pick-files").addEventListener("click", () => void pickFiles());
  $("pick-folder").addEventListener("click", () => void pickFolder());
  $("pick-output").addEventListener("click", () => void pickOutput());
  $("run-btn").addEventListener("click", () => void runPipeline());
  $("clear-log").addEventListener("click", () => {
    $("log").textContent = "";
  });

  for (const id of [
    "assist-provider",
    "assist-api-key",
    "assist-endpoint",
    "assist-model",
  ]) {
    $(id).addEventListener("change", () => void refreshAssistStatus());
    $(id).addEventListener("input", () => void refreshAssistStatus());
  }

  setMode("standard");
  void invoke("app_info")
    .then((info) => appendLog("info", `Ready — ${JSON.stringify(info)}`))
    .catch((err) => appendLog("warn", `UI loaded (invoke pending): ${err}`));
});
