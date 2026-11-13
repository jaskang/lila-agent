//! The project's settings (⇧⌘,), a compact dialog: its name and what a new session starts with.
//! They are `.lila/project.toml`; lilad is its only writer (`project.set`).

use crate::app::LilaApp;
use crate::data::{self, Project};
use crate::dialog::{self, Shell, group, row, value_text};
use crate::engines;
use crate::style::Control as _;
use gpui::component::button::Button;
use gpui::component::input::{Input, InputState};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::json;

pub struct Form {
    pub project: Project,
    pub title: Entity<InputState>,
    /// Empty: omp's own default.
    pub model: String,
    pub permission: String,
    pub error: Option<String>,
    pub saving: bool,
}

impl Form {
    fn new(p: Project, title: Entity<InputState>) -> Form {
        Form { model: p.model.clone(), permission: p.permission.clone(), title, project: p, error: None, saving: false }
    }
}

impl LilaApp {
    pub fn open_settings(&mut self, project: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.project(&project) else { return };
        self.prefs = false;
        self.ensure_models(cx);
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
            "defaults": {"model": f.model, "permission": f.permission},
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

    /// A button showing the model chosen, opening the list omp offers with the chosen one ticked.
    fn model_choice(&self, now: &str, cx: &mut Context<Self>) -> AnyElement {
        let mut items: Vec<(String, String)> = vec![(String::new(), "默认".to_owned())];
        items.extend(self.models.iter().cloned());
        // A model already set stays offered, though omp does not list it.
        if !now.is_empty() && !items.iter().any(|(id, _)| id == now) {
            items.push((now.to_owned(), engines::short_model(now)));
        }
        let label = if now.is_empty() { "默认".to_owned() } else { engines::short_model(now) };
        let (weak, now) = (cx.entity().downgrade(), now.to_owned());
        Button::new("settings-model")
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                for (value, label) in &items {
                    let (weak, value2) = (weak.clone(), value.clone());
                    menu = menu.item(PopupMenuItem::new(label.clone()).checked(*value == now).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            if let Some(f) = this.settings.as_mut() {
                                f.model = value2.clone();
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
        let (project, title, model, permission, error, saving) = (f.project.clone(), f.title.clone(), f.model.clone(), f.permission.clone(), f.error.clone(), f.saving);
        let theme = cx.theme().clone();

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

        let model_row = (!self.models.is_empty() || !model.is_empty()).then(|| row("模型", None, self.model_choice(&model, cx), cx));
        let mut defaults: Vec<AnyElement> = model_row.into_iter().collect();
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
                title: "工作区设置".into(),
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
