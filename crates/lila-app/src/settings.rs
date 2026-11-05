//! Project settings in the window: the project's title, and each member's CLI,
//! model, effort and prompt. Saved with `lila::settings` into the repo's
//! `.lila/`; running members pick them up when restarted.

use crate::app::LilaApp;
use crate::data::{self, Project};
use crate::ui::s;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::scroll::ScrollableElement;
use gpui::component::tag::Tag;
use gpui::component::{ActiveTheme, Disableable, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::settings::{MemberSettings, ProjectSettings};
use lila_core::Runtime;
use serde_json::json;

/// Models offered per CLI; any other can be typed in.
const CLAUDE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"];
const CODEX_MODELS: &[&str] = &["gpt-6.1-sol", "gpt-6-astra", "gpt-6.1-terra", "gpt-6-luna"];
/// `--effort` (claude) and `model_reasoning_effort` (codex) both take these.
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

fn models(runtime: Runtime) -> &'static [&'static str] {
    match runtime {
        Runtime::Claude => CLAUDE_MODELS,
        Runtime::Codex => CODEX_MODELS,
    }
}

pub struct MemberForm {
    pub base: MemberSettings,
    pub runtime: Runtime,
    pub model: Entity<InputState>,
    pub effort: Option<String>,
    pub prompt: Entity<TextareaState>,
}

pub struct Form {
    pub project: Project,
    pub base: ProjectSettings,
    pub title: Entity<InputState>,
    pub members: Vec<MemberForm>,
    pub error: Option<String>,
    pub saving: bool,
}

/// One member's form as it is drawn (read out so the listeners can borrow the app).
struct MemberSnapshot {
    base: MemberSettings,
    runtime: Runtime,
    model: String,
    effort: Option<String>,
    model_input: Entity<InputState>,
    prompt: Entity<TextareaState>,
}

fn field(title: &str, cx: &App) -> Div {
    v_flex().gap_1().child(div().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child(title.to_owned()))
}

impl LilaApp {
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project() else { return };
        let base = match lila::settings::load(std::path::Path::new(&project.cwd), &self.paths().roles()) {
            Ok(b) => b,
            Err(e) => {
                self.notice = Some(format!("读取项目设置失败：{e:#}"));
                cx.notify();
                return;
            }
        };
        self.draft = None;
        self.close_task(cx);
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(base.name.clone()).default_value(base.title.clone()));
        let members = base
            .members
            .iter()
            .map(|m| MemberForm {
                base: m.clone(),
                runtime: m.runtime,
                model: cx.new(|cx| InputState::new(window, cx).placeholder("模型名").default_value(m.model.clone())),
                effort: m.effort.clone(),
                prompt: cx.new(|cx| TextareaState::new(window, cx).rows(10).default_value(m.prompt.clone())),
            })
            .collect();
        self.settings = Some(Form { project, base, title, members, error: None, saving: false });
        cx.notify();
    }

    fn settings_member(&mut self, ix: usize) -> Option<&mut MemberForm> {
        self.settings.as_mut().and_then(|f| f.members.get_mut(ix))
    }

    fn pick_runtime(&mut self, ix: usize, runtime: Runtime, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.settings_member(ix) else { return };
        if m.runtime == runtime {
            return;
        }
        m.runtime = runtime;
        // The model must be one that CLI runs: the role's if it uses this CLI, else the first offered.
        let model = if m.base.role_runtime == runtime { m.base.role_model.clone() } else { models(runtime)[0].to_owned() };
        let input = m.model.clone();
        input.update(cx, |s, cx| s.set_value(model, window, cx));
        cx.notify();
    }

    fn pick_model(&mut self, ix: usize, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.settings_member(ix) else { return };
        let input = m.model.clone();
        input.update(cx, |s, cx| s.set_value(model.to_owned(), window, cx));
        cx.notify();
    }

    fn reset_prompt(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.settings_member(ix) else { return };
        let (input, text) = (m.prompt.clone(), m.base.role_prompt.clone());
        input.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    /// What the form says now, as settings to save.
    fn settings_value(&self, cx: &App) -> Option<(Project, ProjectSettings)> {
        let f = self.settings.as_ref()?;
        let mut out = f.base.clone();
        out.title = f.title.read(cx).value().trim().to_owned();
        for (m, form) in out.members.iter_mut().zip(&f.members) {
            m.runtime = form.runtime;
            m.model = form.model.read(cx).value().trim().to_owned();
            m.effort = form.effort.clone();
            m.prompt = form.prompt.read(cx).value().to_string();
        }
        Some((f.project.clone(), out))
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let Some((project, value)) = self.settings_value(cx) else { return };
        if let Some(f) = &mut self.settings {
            f.saving = true;
            f.error = None;
        }
        cx.notify();
        let paths = self.paths().clone();
        let daemon = self.snap.daemon;
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    lila::settings::save(std::path::Path::new(&project.cwd), &paths.roles(), &value)?;
                    // lilad reads it again; members already running are marked until restarted.
                    // Saved either way: a lilad that can't reload picks it up when it restarts.
                    let warn = if daemon { data::act(&paths, &project, "project.reload", json!({}), false).err() } else { None };
                    anyhow::Ok(warn.map(|e| format!("设置已保存，但 lilad 没能重新读取（{e:#}）。重启 lilad 后生效：lila daemon stop，再启动团队。")))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(warn) => {
                        this.settings = None;
                        this.notice = warn;
                    }
                    Err(e) => {
                        if let Some(f) = &mut this.settings {
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

    pub fn settings_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let f = self.settings.as_ref()?;
        let (title, error, saving, name, label) = (f.title.clone(), f.error.clone(), f.saving, f.base.name.clone(), f.project.label());
        let running: Vec<String> = self.members().iter().filter(|m| m["status"] != "gone").map(|m| s(m, "id")).collect();
        let rows: Vec<(usize, MemberSnapshot)> = f
            .members
            .iter()
            .enumerate()
            .map(|(ix, m)| (ix, MemberSnapshot { base: m.base.clone(), runtime: m.runtime, model: m.model.read(cx).value().to_string(), effort: m.effort.clone(), model_input: m.model.clone(), prompt: m.prompt.clone() }))
            .collect();

        let member_cards: Vec<AnyElement> = rows
            .into_iter()
            .map(|(ix, MemberSnapshot { base, runtime, model, effort, model_input, prompt })| {
                let own_prompt = prompt.read(cx).value().trim() != base.role_prompt.trim();
                let changed = runtime != base.role_runtime || model.trim() != base.role_model || effort != base.role_effort || own_prompt;
                let is_running = running.iter().any(|r| *r == base.id || r.starts_with(&format!("{}-", base.id)));
                let chip = |id: String, text: String, on: bool| Button::new(SharedString::from(id)).xsmall().when(on, |b| b.primary()).when(!on, |b| b.outline()).label(text);
                v_flex()
                    .p_3()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_semibold().child(base.id.clone()))
                            .when(base.count > 1, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("×{}", base.count))))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("角色：{}", base.role_name)))
                            .child(div().flex_1())
                            .when(changed, |d| d.child(Tag::info().xsmall().child("已改"))),
                    )
                    .child(
                        field("运行方式", cx).child(
                            h_flex()
                                .gap_1()
                                .child(chip(format!("rt-{ix}-claude"), "Claude Code".into(), runtime == Runtime::Claude).on_click(cx.listener(move |this, _, window, cx| this.pick_runtime(ix, Runtime::Claude, window, cx))))
                                .child(chip(format!("rt-{ix}-codex"), "Codex".into(), runtime == Runtime::Codex).on_click(cx.listener(move |this, _, window, cx| this.pick_runtime(ix, Runtime::Codex, window, cx)))),
                        ),
                    )
                    .child(
                        field("模型", cx)
                            .child(h_flex().gap_1().flex_wrap().children(models(runtime).iter().map(|m| {
                                let pick = *m;
                                chip(format!("model-{ix}-{m}"), m.to_string(), model.trim() == *m).on_click(cx.listener(move |this, _, window, cx| this.pick_model(ix, pick, window, cx)))
                            })))
                            .child(Input::new(&model_input).small()),
                    )
                    .child(
                        field("努力程度", cx).child(
                            h_flex()
                                .gap_1()
                                .flex_wrap()
                                .child(chip(format!("effort-{ix}-default"), "CLI 默认".into(), effort.is_none()).on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(m) = this.settings_member(ix) {
                                        m.effort = None;
                                    }
                                    cx.notify();
                                })))
                                .children(EFFORTS.iter().map(|e| {
                                    let pick = e.to_string();
                                    chip(format!("effort-{ix}-{e}"), e.to_string(), effort.as_deref() == Some(*e)).on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(m) = this.settings_member(ix) {
                                            m.effort = Some(pick.clone());
                                        }
                                        cx.notify();
                                    }))
                                })),
                        ),
                    )
                    .child(
                        field("预设提示词", cx)
                            .child(Textarea::new(&prompt))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if own_prompt { format!("只给这个成员用，保存在 .lila/prompts/{}.md", base.id) } else { format!("和角色「{}」的默认提示词一样", base.role_name) })
                                    .child(div().flex_1())
                                    .when(own_prompt, |d| d.child(Button::new(SharedString::from(format!("prompt-reset-{ix}"))).xsmall().ghost().label("恢复角色默认").on_click(cx.listener(move |this, _, window, cx| this.reset_prompt(ix, window, cx))))),
                            ),
                    )
                    .when(changed && is_running, |d| d.child(div().text_xs().text_color(theme.yellow).child("它正在运行：保存后要重启它才会用上新设置。")))
                    .into_any_element()
            })
            .collect();

        Some(
            v_flex()
                .id("settings")
                .size_full()
                .text_color(theme.popover_foreground)
                .child(
                    h_flex()
                        .h(px(40.))
                        .px_3()
                        .flex_none()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(div().font_semibold().child("项目设置"))
                        .child(div().text_sm().text_color(theme.muted_foreground).truncate().child(label))
                        .child(div().flex_1())
                        .child(Button::new("settings-close").small().ghost().label("关闭").on_click(cx.listener(|this, _, _, cx| {
                            this.settings = None;
                            cx.notify();
                        }))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .p_4()
                        .gap_4()
                        .child(
                            field("项目名称", cx)
                                .child(Input::new(&title))
                                .child(div().text_xs().text_color(theme.muted_foreground).child(format!("窗口里显示的名字，可以用中文。项目编号 {name} 不变，命令行和终端名用的是它。"))),
                        )
                        .child(div().text_xs().font_semibold().text_color(theme.muted_foreground).child("成员"))
                        .children(member_cards)
                        .child(div().text_xs().text_color(theme.muted_foreground).child("只保存和角色默认值不同的部分，写在仓库的 .lila/project.toml 里，可以随代码一起提交。")),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .p_3()
                        .gap_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .when_some(error, |d, e| d.child(div().p_2().rounded_md().border_l_2().border_color(theme.red).bg(theme.red.opacity(0.12)).text_sm().child(e)))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("settings-save").primary().label(if saving { "保存中…" } else { "保存" }).disabled(saving).on_click(cx.listener(|this, _, _, cx| this.save_settings(cx))))
                                .child(Button::new("settings-cancel").ghost().label("取消").on_click(cx.listener(|this, _, _, cx| {
                                    this.settings = None;
                                    cx.notify();
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }
}
