# MANUCRAFT customizations (PhotoCraft)

This fork (`netoonine/photocraft`, branch `manucraft`) = upstream `storytold/photocraft` + the changes below.
Every line we add to an upstream file is marked `MANUCRAFT` (`grep -rn MANUCRAFT crates Cargo.toml`).
When a sync conflicts, keep upstream's version of the surrounding code and re-apply our marked lines.

## 1. Generative AI through KIE

**What:** Edit › Generate Image…, Edit › Generative Fill… (With context / Selection only), Edit › AI API Key…;
KIE AI through the shared crate `../craft-ai`, with a model picker (13 models from `craft_ai::models::IMAGE_MODELS`:
Nano Banana 2/2.1/Pro/Lite, GPT Image 2/2.5, Seedream 5 Pro/Flash, Flux 2 Pro, Qwen 3 Pro, Grok Imagine 2, Wan 2.7 Pro,
Imagen 4 Ultra; `nano-banana-2` by default). Reference images are scaled to 2048 px before upload. Key in the macOS Keychain.

**Our files (never conflict):**
- `crates/engine/src/genai_cmds.rs`: commands `edit.generateImage`, `edit.generativeFill`
  (`context` true/false), `ai.models` (the picker's list), `ai.setApiKey`, `ai.clearApiKey`, `ai.status`; `disable_network()`.
- `crates/ui-egui/src/genai_ui.rs`: the dialogs; `GenAiLast`; `models`/`model_options`/`fit_model`/`model_dropdown` (shared with the context bar).
- `crates/ui-egui/src/i18n/manucraft/` (`mod.rs` + `*.tsv`): our translations (20 languages), merged when a catalog is parsed; own consistency test.

**Hooks in upstream files:**

| File | Hook |
|---|---|
| `Cargo.toml` | workspace dep `craft-ai = { path = "../craft-ai" }` |
| `crates/engine/Cargo.toml` | `craft-ai` under `cfg(not(target_arch = "wasm32"))` |
| `crates/engine/src/lib.rs` | `pub mod genai_cmds;` |
| `crates/engine/src/commands.rs` | `v.extend(crate::genai_cmds::specs());` (after `cutout_cmds`) |
| `crates/engine/tests/panic_hunt.rs` | `genai_cmds::disable_network()` in `fresh()` |
| `crates/ui-egui/src/lib.rs` | `pub mod genai_ui;` |
| `crates/ui-egui/src/menu_catalog.rs` | 3 Edit rows after Content-Aware Fill |
| `crates/ui-egui/src/menus.rs` | `genai_ui::intercepts` arm in `invoke_unguarded` (opens the dialogs) |
| `crates/ui-egui/src/dialogs.rs` | width, `body` and `confirm` routing for `genai_ui::owns` |
| `crates/ui-egui/src/state.rs` | `UiState.genai_last` (+ its `Default`) |
| `crates/ui-egui/src/i18n/mod.rs` | `mod manucraft;` + `LangInfo::catalog` parses `text() + manucraft::extra(code)` |
| `Cargo.lock` | regenerate (`cargo update -p craft-ai` or any build) — never hand-merge |

**Test:** `cargo test -p photocraft-engine --lib genai`, `cargo test -p photocraft-ui-egui --lib genai i18n`;
live: `photocraft-cli run --new '{"width":800,"height":450}' --cmd edit.generateImage --params '{"prompt":"a lighthouse"}' --out /tmp/g.png`.

## 2. Contextual task bar

**What:** a floating bar, docked at the bottom centre of the canvas while there is a selection, with “Generate with AI” (With context / Selection only);
Window › Contextual Task Bar toggles it.

**Our files:** `crates/ui-egui/src/context_bar.rs` (`ContextBarUi`, `show`, `target`, `params`).

**Hooks:**

| File | Hook |
|---|---|
| `crates/ui-egui/src/lib.rs` | `pub mod context_bar;` |
| `crates/ui-egui/src/canvas.rs` | `crate::context_bar::show(app, ui.ctx());` at the end of `document_area` (single view); the bar is docked at the canvas bottom centre so right-clicks near the selection still open canvas menus |
| `crates/ui-egui/src/state.rs` | `Panels.context_bar` (serde default yes) + `UiState.context_bar` (+ `Default`s) |
| `crates/engine/src/shell_commands.rs` | `SHELL_COMMANDS` row `window.toggle.contextBar` (upstream moved `UI_COMMANDS` here in 2026-10) |
| `crates/ui-egui/src/menus.rs` | toggle arm; checkmark arm |

**Test:** `cargo test -p photocraft-ui-egui --lib context_bar`; visual: `cargo run -p photocraft-ui-egui --example snapshot`
with `select.rect`, then look at the bottom centre of the canvas.
