//! The project's settings (⇧⌘,), a compact dialog: its name and what a new session starts with.
//! They are `.lila/project.toml`; lilad is its only writer (`project.set`).

use crate::app::LilaApp;
use crate::data::{self, Project};
use crate::dialog::{self, Shell, group, row, value_text};
use crate::engines;
use gpui::component::button::Button;
use gpui::component::input::{Input, InputState};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use serde_json::json;

pub struct Form {
    pub project: Project,
    pub title: Entity<InputState>,
    pub engine: String,
    /// Empty: the engine's own default.
    pub model: String,
    /// Empty: the engine's own default.
    pub effort: String,
    pub permission: String,
    pub error: Option<String>,
    pub saving: bool,
}

impl Form {
    fn new(p: Project, title: Entity<InputState>) -> Form {
        // A model that is the engine's default is stored as no model: the project follows the engine.
        let model = if p.model == engines::default_model(&p.engine) { String::new() } else { p.model.clone() };
        Form { engine: p.engine.clone(), model, effort: p.effort.clone(), permission: p.permission.clone(), title, project: p, error: None, saving: false }
    }

    fn set_engine(&mut self, engine: &str) {
        if self.engine != engine {
            self.engine = engine.to_owned();
            self.model.clear();
            self.effort.clear();
        }
    }
}

impl LilaApp {
    pub fn open_settings(&mut self, project: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.project(&project) else { return };
        self.prefs = None;
        self.dialog_offset = Point::default();
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(p.id.clone()).default_value(p.title.clone()));
        self.settings = Some(Form::new(p, title));
        cx.notify();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.settings.as_mut() else { return };
        f.saving = true;
        f.error = None;
        let params = json!({
            "project": f.project.id,
            "title": f.title.read(cx).value().trim(),
            "defaults": {"engine": f.engine, "model": f.model, "effort": f.effort, "permission": f.permission},
        });
        cx.notify();
        let paths = self.paths().clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, "project.set", params, false) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(_) => this.settings = None,
                    Err(e) => {
                        if let Some(f) = this.settings.as_mut() {
                            f.saving = false;
                            f.error = Some(format!("{e:#}"));
                        }
                    }
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
    }

    /// A button showing the choice made, opening the list to choose from: `(value, label)` with the chosen one ticked.
    fn choice(&self, id: &'static str, now: &str, items: Vec<(String, String)>, set: impl Fn(&mut Form, &str) + Clone + 'static, cx: &mut Context<Self>) -> AnyElement {
        let label = items.iter().find(|(v, _)| v == now).map_or(now.to_owned(), |(_, l)| l.clone());
        let (weak, now) = (cx.entity().downgrade(), now.to_owned());
        Button::new(id)
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                for (value, label) in &items {
                    let (weak, set, value2) = (weak.clone(), set.clone(), value.clone());
                    menu = menu.item(PopupMenuItem::new(label.clone()).checked(*value == now).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            if let Some(f) = this.settings.as_mut() {
                                set(f, &value2);
                            }
                            cx.notify();
                        });
                    }));
                }
                menu
            })
            .into_any_element()
    }

    pub fn settings_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.settings.as_ref()?;
        let (project, title, engine, model, effort, permission, error, saving) = (f.project.clone(), f.title.clone(), f.engine.clone(), f.model.clone(), f.effort.clone(), f.permission.clone(), f.error.clone(), f.saving);
        let theme = cx.theme().clone();

        let engines_row = dialog::segmented(
            "settings-engine",
            &engines::ENGINES.map(|(_, n)| n),
            engines::ENGINES.iter().position(|(id, _)| *id == engine).unwrap_or(0),
            |this, i, _, _| {
                if let Some(f) = this.settings.as_mut() {
                    f.set_engine(engines::ENGINES[i].0);
                }
            },
            cx,
        );
        // Models: the engine's own default first, then those it is offered (a model already set stays offered).
        let mut models: Vec<(String, String)> = vec![(String::new(), engines::short_model(engines::default_model(&engine)))];
        let mut offered: Vec<String> = engines::models(&engine).iter().map(|m| m.to_string()).collect();
        if !model.is_empty() && !offered.contains(&model) {
            offered.push(model.clone());
        }
        models.extend(offered.into_iter().filter(|m| *m != engines::default_model(&engine)).map(|m| (m.clone(), engines::short_model(&m))));
        let model_row = (!engines::models(&engine).is_empty() || !model.is_empty()).then(|| row("模型", None, self.choice("settings-model", &model, models, |f, v| f.model = v.to_owned(), cx), cx));
        let efforts: Vec<(String, String)> = std::iter::once(("", "默认")).chain(engines::EFFORTS.iter().map(|e| (*e, *e))).map(|(v, l)| (v.to_owned(), l.to_owned())).collect();
        let effort_row = engines::has_effort(&engine).then(|| row("努力程度", None, self.choice("settings-effort", &effort, efforts, |f, v| f.effort = v.to_owned(), cx), cx));
        let tier_row = dialog::segmented(
            "settings-tier",
            &engines::TIERS.map(|(_, n, _)| n),
            engines::TIERS.iter().position(|(id, ..)| *id == permission).unwrap_or(2),
            |this, i, _, _| {
                if let Some(f) = this.settings.as_mut() {
                    f.permission = engines::TIERS[i].0.to_owned();
                }
            },
            cx,
        );

        let mut defaults = vec![row("引擎", None, engines_row, cx)];
        defaults.extend(model_row);
        defaults.extend(effort_row);
        defaults.push(row("权限", None, tier_row, cx));
        let body = dialog::page(vec![
            group(None, vec![row("名称", None, div().w(px(280.)).child(Input::new(&title).control()), cx), row("文件夹", None, value_text(data::tail(&data::tilde(&project.cwd), 40), cx), cx)], cx),
            group(None, defaults, cx),
        ]);
        let note = error.map(|e| h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element());
        Some(self.dialog(
            Shell {
                id: "project-settings",
                nav: Vec::new(),
                title: "项目设置".into(),
                body,
                note,
                saving,
                save_label: if saving { "保存中…" } else { "保存" },
                can_save: true,
                on_save: Box::new(|this, cx| this.save_settings(cx)),
                on_close: Box::new(|this, _| this.settings = None),
                footer: true,
                compact: true,
            },
            cx,
        ))
    }
}
