//! What a new session can be asked for, with the words the window calls them. Lilad decides what is valid; these only
//! fill the menus.

/// (id, name, icon): the permission tiers, least to most trusting.
pub const TIERS: [(&str, &str, &str); 3] = [("read_only", "只读", "lila/lock.svg"), ("confirm", "改动前确认", "lila/shield-check.svg"), ("full", "全自动", "lila/zap.svg")];

pub fn tier_name(tier: &str) -> &'static str {
    TIERS.iter().find(|(id, ..)| *id == tier).map_or("全自动", |(_, n, _)| n)
}

pub fn tier_icon(tier: &str) -> &'static str {
    TIERS.iter().find(|(id, ..)| *id == tier).map_or("lila/zap.svg", |(.., i)| i)
}

/// `openai/gpt-6` as `gpt-6`: what fits a button.
pub fn short_model(model: &str) -> String {
    model.rsplit('/').next().unwrap_or(model).to_owned()
}

/// A model and effort as one phrase: `gpt-6 · high`; `默认` for omp's own model when none is named.
pub fn spec_label(model: &str, effort: &str) -> String {
    let model = if model.is_empty() { "默认".to_owned() } else { short_model(model) };
    if effort.is_empty() { model } else { format!("{model} · {effort}") }
}

/// `text` cut to `max` characters with an ellipsis, for a label that has to fit a button or a menu row.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_owned() } else { format!("{}…", text.chars().take(max.saturating_sub(1)).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_read_short_and_omps_default_is_said_so() {
        assert_eq!(short_model("openai/gpt-6"), "gpt-6");
        assert_eq!(short_model("gpt-6"), "gpt-6");
        assert_eq!(spec_label("openai/gpt-6", "high"), "gpt-6 · high");
        assert_eq!(spec_label("gpt-6-luna", ""), "gpt-6-luna");
        assert_eq!(spec_label("", ""), "默认");
        assert!(TIERS.iter().all(|(id, ..)| lila_core::Tier::parse(id).is_some()));
    }

    #[test]
    fn a_long_label_is_cut_to_fit_and_a_short_one_is_left() {
        assert_eq!(clip("本会话总是允许 cargo test", 40), "本会话总是允许 cargo test");
        assert_eq!(clip("0123456789", 6), "01234…");
        assert_eq!(clip("字字字字字", 5), "字字字字字");
        assert_eq!(clip("字字字字字字", 5), "字字字字…");
        assert_eq!(clip("", 0), "");
    }
}
