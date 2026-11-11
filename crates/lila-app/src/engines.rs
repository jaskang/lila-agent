//! What a new session can be asked for: the engines, the models each is offered, the efforts and the
//! permission tiers, with the words the window calls them. Lilad decides what is valid; these only
//! fill the menus.

/// (id, name): the engines a thread can run on.
pub const ENGINES: [(&str, &str); 3] = [("claude", "Claude Code"), ("codex", "Codex"), ("omp", "omp")];

/// (id, name, icon): the permission tiers, least to most trusting.
pub const TIERS: [(&str, &str, &str); 3] = [("read_only", "只读", "lila/lock.svg"), ("confirm", "改动前确认", "lila/shield-check.svg"), ("full", "全自动", "lila/zap.svg")];

/// `--effort` (Claude Code) and `model_reasoning_effort` (Codex) take these.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

const CLAUDE_MODELS: [&str; 4] = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"];
const CODEX_MODELS: [&str; 4] = ["gpt-6.1-sol", "gpt-6-astra", "gpt-6.1-terra", "gpt-6-luna"];

pub fn engine_name(engine: &str) -> &'static str {
    ENGINES.iter().find(|(id, _)| *id == engine).map_or("Claude Code", |(_, n)| n)
}

/// The models offered for an engine (omp lists its own once it runs; before that, only its default).
pub fn models(engine: &str) -> &'static [&'static str] {
    match engine {
        "claude" => &CLAUDE_MODELS,
        "codex" => &CODEX_MODELS,
        _ => &[],
    }
}

/// Does the engine take an effort at launch?
pub fn has_effort(engine: &str) -> bool {
    engine != "omp"
}

/// The model an engine starts on when none is chosen (`lila_core::Runtime::default_model`).
pub fn default_model(engine: &str) -> &'static str {
    lila_core::Runtime::parse(engine).map_or("", lila_core::Runtime::default_model)
}

pub fn tier_name(tier: &str) -> &'static str {
    TIERS.iter().find(|(id, ..)| *id == tier).map_or("全自动", |(_, n, _)| n)
}

pub fn tier_icon(tier: &str) -> &'static str {
    TIERS.iter().find(|(id, ..)| *id == tier).map_or("lila/zap.svg", |(.., i)| i)
}

/// `claude-sonnet-5-5` as `sonnet-5-5`, `openai/gpt-6` as `gpt-6`: what fits a button.
pub fn short_model(model: &str) -> String {
    let m = model.rsplit('/').next().unwrap_or(model);
    m.strip_prefix("claude-").unwrap_or(m).to_owned()
}

/// The model as a button shows it; the engine's default when none is named.
fn model_name(engine: &str, model: &str) -> String {
    let model = if model.is_empty() { default_model(engine) } else { model };
    if model.is_empty() { "默认".to_owned() } else { short_model(model) }
}

/// A model and effort as one phrase: `sonnet-5-5 · high`; the engine's default model when none is named.
pub fn spec_label(engine: &str, model: &str, effort: &str) -> String {
    let model = model_name(engine, model);
    if effort.is_empty() { model } else { format!("{model} · {effort}") }
}

/// What the box's model button says: the model, then the engine and its effort (`sonnet-5-5 · Claude Code high`).
pub fn engine_spec(engine: &str, model: &str, effort: &str) -> String {
    let (model, engine) = (model_name(engine, model), engine_name(engine));
    if effort.is_empty() { format!("{model} · {engine}") } else { format!("{model} · {engine} {effort}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_read_short_and_the_default_fills_in() {
        assert_eq!(short_model("claude-sonnet-5-5"), "sonnet-5-5");
        assert_eq!(short_model("openai/gpt-6"), "gpt-6");
        assert_eq!(spec_label("claude", "", "high"), "sonnet-5-5 · high");
        assert_eq!(spec_label("codex", "gpt-6-luna", ""), "gpt-6-luna");
        assert_eq!(spec_label("omp", "", ""), "默认");
        assert_eq!(engine_spec("claude", "", "high"), "sonnet-5-5 · Claude Code high");
        assert_eq!(engine_spec("omp", "", ""), "默认 · omp");
        assert!(ENGINES.iter().all(|(id, _)| lila_core::Runtime::parse(id).is_some()));
        assert!(TIERS.iter().all(|(id, ..)| lila_core::Tier::parse(id).is_some()));
    }
}
