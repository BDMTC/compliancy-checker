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

type IsoFirstResult = {
  ok: boolean;
  passed: boolean;
  standardsLabels: string[];
  passMessage: string;
  reasons: string[];
  csv: string;
  reportText: string;
  combinedReport: string;
  filesWritten: string[];
  log: RunLogLine[];
};

let isoPrintReport = "";
let passToastTimer: number | undefined;

type DetectedTrait = {
  id: string;
  kind: "font" | "colour" | "background";
  label: string;
  value: string;
  styleId?: string | null;
  swatch?: string | null;
};

type TemplatePreview = {
  path: string;
  fileName: string;
  format?: string;
  traits: DetectedTrait[];
  summary: {
    fonts: string[];
    colours: string[];
    backgroundFill?: string | null;
    hasBackgroundChrome: boolean;
    styleCount: number;
  };
  limits?: string[];
};

const state = {
  mode: "standard" as Mode,
  inputPaths: [] as string[],
  templatePreview: null as TemplatePreview | null,
  /** When non-null, only these trait ids are applied. Null means “all”. */
  selectedTraitIds: null as string[] | null,
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

function kindLabel(kind: DetectedTrait["kind"]): string {
  switch (kind) {
    case "font":
      return "Fonts";
    case "colour":
      return "Colours";
    case "background":
      return "Backgrounds";
  }
}

function renderTemplatePreview(preview: TemplatePreview | null) {
  const panel = $("template-preview");
  const limitsEl = $("template-limits");
  if (!preview || preview.traits.length === 0) {
    state.templatePreview = null;
    state.selectedTraitIds = null;
    panel.hidden = true;
    panel.classList.add("is-empty");
    $("trait-list").textContent = "";
    $("template-swatches").textContent = "";
    $("template-preview-meta").textContent = "";
    limitsEl.hidden = true;
    limitsEl.textContent = "";
    return;
  }

  state.templatePreview = preview;
  // Default: all traits selected.
  state.selectedTraitIds = preview.traits.map((t) => t.id);

  panel.hidden = false;
  panel.classList.remove("is-empty");
  const formatLabel = (preview.format || "docx").toUpperCase();
  $("template-preview-title").textContent = `${preview.fileName} (${formatLabel})`;

  const swatches = $("template-swatches");
  swatches.textContent = "";
  const seen = new Set<string>();
  for (const t of preview.traits) {
    const hex = t.swatch?.toUpperCase();
    if (!hex || seen.has(hex)) continue;
    seen.add(hex);
    const chip = document.createElement("span");
    chip.className = "swatch";
    chip.style.setProperty("--swatch", `#${hex}`);
    chip.title = `#${hex}`;
    chip.setAttribute("aria-label", `Colour #${hex}`);
    swatches.appendChild(chip);
  }

  const fonts = preview.summary.fonts.slice(0, 4).join(", ") || "—";
  const chrome = preview.summary.hasBackgroundChrome
    ? "behind-doc chrome"
    : "no chrome";
  const fill = preview.summary.backgroundFill
    ? `#${preview.summary.backgroundFill}`
    : "no page fill";
  const unit =
    preview.format === "pdf" ? "page(s)" : "style(s)";
  $("template-preview-meta").textContent =
    `${preview.traits.length} trait(s) · ${preview.summary.styleCount} ${unit} · fonts: ${fonts} · ${fill} · ${chrome}`;

  const list = $("trait-list");
  list.textContent = "";

  const order: DetectedTrait["kind"][] = ["background", "font", "colour"];
  for (const kind of order) {
    const groupTraits = preview.traits.filter((t) => t.kind === kind);
    if (groupTraits.length === 0) continue;

    const group = document.createElement("div");
    group.className = "trait-group";
    const heading = document.createElement("p");
    heading.className = "trait-group-label";
    heading.textContent = kindLabel(kind);
    group.appendChild(heading);

    for (const trait of groupTraits) {
      const label = document.createElement("label");
      label.className = "trait-item";
      const input = document.createElement("input");
      input.type = "checkbox";
      input.value = trait.id;
      input.checked = true;
      input.addEventListener("change", syncTraitSelectionFromDom);
      const text = document.createElement("span");
      text.className = "trait-text";
      text.textContent = `${trait.label}: ${trait.value}`;
      label.appendChild(input);
      if (trait.swatch) {
        const dot = document.createElement("span");
        dot.className = "swatch swatch-sm";
        dot.style.setProperty("--swatch", `#${trait.swatch}`);
        label.appendChild(dot);
      }
      label.appendChild(text);
      group.appendChild(label);
    }
    list.appendChild(group);
  }

  limitsEl.textContent = "";
  if (preview.limits && preview.limits.length > 0) {
    limitsEl.hidden = false;
    for (const limit of preview.limits) {
      const li = document.createElement("li");
      li.textContent = limit;
      limitsEl.appendChild(li);
    }
  } else {
    limitsEl.hidden = true;
  }
}

function syncTraitSelectionFromDom() {
  if (!state.templatePreview) {
    state.selectedTraitIds = null;
    return;
  }
  const boxes = document.querySelectorAll<HTMLInputElement>(
    "#trait-list input[type=checkbox]",
  );
  state.selectedTraitIds = Array.from(boxes)
    .filter((b) => b.checked)
    .map((b) => b.value);
}

function setAllTraits(checked: boolean) {
  const boxes = document.querySelectorAll<HTMLInputElement>(
    "#trait-list input[type=checkbox]",
  );
  for (const box of boxes) {
    box.checked = checked;
  }
  syncTraitSelectionFromDom();
}

async function loadTemplatePreview(path: string) {
  try {
    appendLog("info", `Inspecting template traits: ${path}`);
    const preview = await invoke<TemplatePreview>("inspect_template", { path });
    renderTemplatePreview(preview);
    appendLog(
      "info",
      `Detected ${preview.traits.length} trait(s) in ${preview.fileName} — select all, one, or a subset before Run.`,
    );
  } catch (err) {
    renderTemplatePreview(null);
    appendLog("error", `Template inspect failed: ${err}`);
  }
}

async function pickTemplate() {
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [
      { name: "Templates", extensions: ["docx", "pdf"] },
      { name: "Word", extensions: ["docx"] },
      { name: "PDF", extensions: ["pdf"] },
    ],
  });
  if (typeof selected === "string") {
    ($("template-path") as HTMLInputElement).value = selected;
    await loadTemplatePreview(selected);
  }
}

function selectedOutputFormat(): "match" | "docx" | "pdf" {
  const checked = document.querySelector<HTMLInputElement>(
    'input[name="output-format"]:checked',
  );
  const v = checked?.value;
  if (v === "pdf") return "pdf";
  if (v === "docx") return "docx";
  return "match";
}

function appendInputPaths(paths: string[]) {
  const seen = new Set(state.inputPaths);
  for (const p of paths) {
    if (!seen.has(p)) {
      state.inputPaths.push(p);
      seen.add(p);
    }
  }
  syncInputPathsDisplay();
}

async function pickFiles() {
  const selected = await open({
    multiple: true,
    directory: false,
    filters: [
      { name: "Documents", extensions: ["docx", "pdf", "doc"] },
      { name: "Word", extensions: ["docx", "doc"] },
      { name: "PDF", extensions: ["pdf"] },
    ],
  });
  if (Array.isArray(selected)) {
    appendInputPaths(selected);
  } else if (typeof selected === "string") {
    appendInputPaths([selected]);
  }
}

async function pickFolder() {
  const selected = await open({
    multiple: true,
    directory: true,
  });
  if (Array.isArray(selected)) {
    appendInputPaths(selected);
  } else if (typeof selected === "string") {
    appendInputPaths([selected]);
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

function selectedTraitIdsForRequest(): string[] | null {
  if (!($("template-enabled") as HTMLInputElement).checked) {
    return null;
  }
  if (!state.templatePreview) {
    return null; // backend treats null as “all”
  }
  syncTraitSelectionFromDom();
  return state.selectedTraitIds;
}

function showPassToast(message: string) {
  const toast = $("iso-pass-toast");
  toast.hidden = false;
  toast.textContent = message;
  // Force reflow so the transition replays when shown again.
  void toast.offsetWidth;
  toast.classList.add("is-visible");
  if (passToastTimer !== undefined) {
    window.clearTimeout(passToastTimer);
  }
  passToastTimer = window.setTimeout(() => {
    toast.classList.remove("is-visible");
    window.setTimeout(() => {
      toast.hidden = true;
      toast.textContent = "";
    }, 220);
  }, 4500);
}

function showFailDialog(reasons: string[], combinedReport: string) {
  isoPrintReport = combinedReport;
  const list = $("iso-fail-reasons");
  list.textContent = "";
  for (const reason of reasons) {
    const li = document.createElement("li");
    li.textContent = reason;
    list.appendChild(li);
  }
  $("iso-fail-summary").textContent =
    reasons.length === 1
      ? "1 missing clause from the ISO checklist:"
      : `${reasons.length} missing clause(s) from the ISO checklist:`;
  const dialog = $("iso-fail-dialog") as HTMLDialogElement;
  if (typeof dialog.showModal === "function") {
    dialog.showModal();
  } else {
    dialog.setAttribute("open", "");
  }
}

function printCombinedIsoReport() {
  const existing = document.getElementById("iso-print-root");
  if (existing) {
    existing.remove();
  }
  const root = document.createElement("pre");
  root.id = "iso-print-root";
  root.textContent =
    isoPrintReport || "No ISO RUN FIRST report available to print.";
  document.body.appendChild(root);
  window.print();
  window.setTimeout(() => root.remove(), 500);
}

async function runIsoFirst() {
  const btn = $("iso-run-first") as HTMLButtonElement;
  btn.disabled = true;
  try {
    const standards = selectedIsoStandards();
    if (standards.length === 0) {
      appendLog("error", "Select at least one ISO standard before RUN FIRST.");
      return;
    }
    if (state.inputPaths.length === 0) {
      appendLog("error", "Choose input files or a folder before RUN FIRST.");
      return;
    }

    const outputDir = ($("output-dir") as HTMLInputElement).value.trim();
    const request = {
      inputPaths: state.inputPaths,
      standards,
      outputDir: outputDir || null,
    };

    appendLog("info", "ISO RUN FIRST — checklist before pipeline…");
    const result = await invoke<IsoFirstResult>("run_iso_first", { request });
    for (const line of result.log) {
      appendLog(line.level, line.message);
    }

    if (result.passed) {
      showPassToast(result.passMessage);
      appendLog("info", result.passMessage);
      return;
    }

    const reasons =
      result.reasons.length > 0
        ? result.reasons
        : ["ISO checklist failed (see run log)."];
    showFailDialog(reasons, result.combinedReport);
    appendLog(
      "warn",
      `ISO RUN FIRST failed with ${reasons.length} reason(s).`,
    );
  } catch (err) {
    appendLog("error", String(err));
  } finally {
    btn.disabled = false;
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
      outputFormat: selectedOutputFormat(),
      iso: {
        enabled: ($("iso-enabled") as HTMLInputElement).checked,
        standards: selectedIsoStandards(),
      },
      template: {
        enabled: ($("template-enabled") as HTMLInputElement).checked,
        templatePath: ($("template-path") as HTMLInputElement).value || null,
        selectedTraitIds: selectedTraitIdsForRequest(),
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
  $("iso-run-first").addEventListener("click", () => void runIsoFirst());
  $("iso-fail-print").addEventListener("click", () => printCombinedIsoReport());
  $("clear-log").addEventListener("click", () => {
    $("log").textContent = "";
  });
  $("traits-select-all").addEventListener("click", () => setAllTraits(true));
  $("traits-select-none").addEventListener("click", () => setAllTraits(false));

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
