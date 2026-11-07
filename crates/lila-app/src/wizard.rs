//! The new-project dialog, one page: the folder, the name, the team (roles
//! and how many of each), and the brief the PM gets as its first task.
//! Creating writes `.lila/project.toml` (unless the folder already is a
//! project), runs `git init` in a folder that isn't a repository yet, and
//! starts the team. The project's id comes from the folder's name.

use crate::app::LilaApp;
use crate::dialog::{Act, Shell, block, group, row, stepper, value_text};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::checkbox::Checkbox;
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use lila::init::NewProject;
use std::path::{Path, PathBuf};

/// A role the team can have.
pub struct Pick {
    pub role: String,
    pub name: String,
    pub on: bool,
    pub count: u32,
    /// Its CLI, when it isn't installed: the role can't be picked.
    pub missing: Option<String>,
}

/// A folder that already is (or is inside) a Lila project: its team is kept.
pub struct Existing {
    pub root: PathBuf,
    /// Why its config doesn't load, if it doesn't.
    pub error: Option<String>,
}

pub struct Folder {
    pub dir: PathBuf,
    pub existing: Option<Existing>,
    /// The id for a new project: the one the database has for this folder
    /// (its config was removed), else one made from the folder's name.
    pub id: String,
    pub git: bool,
}

pub struct Wizard {
    pub folder: Option<Folder>,
    pub title: Entity<InputState>,
    pub picks: Vec<Pick>,
    pub brief: Entity<TextareaState>,
    pub error: Option<String>,
    pub busy: bool,
}

/// What creating does, worked out on the UI thread and run off it.
struct Plan {
    dir: PathBuf,
    git_init: bool,
    new: Option<NewProject>,
    brief: Option<String>,
}

fn run(paths: &lila::paths::Paths, plan: Plan) -> anyhow::Result<String> {
    if plan.git_init {
        let out = std::process::Command::new("git").arg("init").arg("-q").current_dir(&plan.dir).output()?;
        if !out.status.success() {
            anyhow::bail!("git init: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
    }
    // A config already there is an earlier try that got this far: start that one.
    match &plan.new {
        Some(p) if !plan.dir.join(".lila").join("project.toml").exists() => {
            lila::init::create_project(paths, &plan.dir, p)?;
        }
        _ => {
            lila::roles::install_defaults(&paths.roles())?;
        }
    }
    let p = crate::data::Project { cwd: plan.dir.to_string_lossy().to_string(), ..Default::default() };
    let v = crate::data::act(paths, &p, "project.up", serde_json::json!({"brief": plan.brief}), true)?;
    Ok(v["project"].as_str().unwrap_or_default().to_owned())
}

/// A long path as its last `n` characters, so the folder's own name shows.
fn tail(path: &str, n: usize) -> String {
    let count = path.chars().count();
    if count <= n { path.to_owned() } else { format!("…{}", path.chars().skip(count - n + 1).collect::<String>()) }
}

fn is_git(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

impl LilaApp {
    /// Opens the new-project dialog (no folder yet).
    pub fn new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.paths().clone();
        // A fresh machine has no roles yet: put the defaults in place to choose from.
        let _ = lila::roles::install_defaults(&paths.roles());
        let roles = lila::roles::load(&paths.roles()).unwrap_or_default();
        let default_team = ["pm", "designer", "architect", "engineer", "tester"];
        let order = |id: &str| lila::roles::DEFAULTS.iter().position(|(d, _, _)| *d == id).unwrap_or(usize::MAX);
        let mut roles: Vec<_> = roles.into_values().collect();
        roles.sort_by(|a, b| (order(&a.id), &a.id).cmp(&(order(&b.id), &b.id)));
        let path = std::env::var("PATH").unwrap_or_default();
        let picks = roles
            .iter()
            .map(|r| {
                let cli = r.file.runtime.to_string();
                let role_path = r.file.env.get("PATH").map(|p| format!("{p}:{path}")).unwrap_or_else(|| path.clone());
                let missing = lila::paths::which(&cli, &role_path).is_none().then_some(cli);
                Pick { role: r.id.clone(), name: r.file.name.clone(), on: default_team.contains(&r.id.as_str()) && missing.is_none(), count: 1, missing }
            })
            .collect();
        self.settings = None;
        self.prefs = None;
        self.draft = None;
        self.dialog_offset = Point::default();
        self.wizard = Some(Wizard {
            folder: None,
            title: cx.new(|cx| InputState::new(window, cx)),
            picks,
            brief: cx.new(|cx| TextareaState::new(window, cx).rows(7).placeholder("想做什么？")),
            error: None,
            busy: false,
        });
        cx.notify();
    }

    /// Asks for the folder, then fills in the first step from it.
    pub fn wizard_pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("用这个文件夹".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(dirs))) = picked.await else { return };
            let Some(dir) = dirs.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| this.wizard_set_folder(dir, window, cx));
        })
        .detach();
    }

    pub fn wizard_set_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let roles = self.paths().roles();
        let dir_s = dir.to_string_lossy().to_string();
        let existing = lila::paths::find_project_root(&dir).map(|root| Existing { error: lila::settings::load(&root, &roles).err().map(|e| format!("读不了这个项目的配置：{e:#}")), root });
        let folder_name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // The folder's name as an id, made unique among the other folders' projects.
        let id = self.snap.projects.iter().find(|p| p.cwd == dir_s).map(|p| p.id.clone()).unwrap_or_else(|| {
            let base = lila::init::slug(&folder_name);
            let taken = |id: &str| self.snap.projects.iter().any(|p| p.id == id);
            std::iter::once(base.clone()).chain((2..).map(|n| format!("{base}-{n}"))).find(|id| !taken(id)).expect("a free id")
        });
        let Some(w) = self.wizard.as_mut() else { return };
        w.title.update(cx, |s, cx| s.set_value(folder_name, window, cx));
        w.error = existing.as_ref().and_then(|e| e.error.clone());
        w.folder = Some(Folder { git: is_git(&dir), dir, existing, id });
        cx.notify();
    }

    fn wizard_plan(&self, cx: &App) -> Option<Plan> {
        let w = self.wizard.as_ref()?;
        let f = w.folder.as_ref()?;
        let brief = w.brief.read(cx).value().trim().to_owned();
        let new = f.existing.is_none().then(|| NewProject {
            name: f.id.clone(),
            title: w.title.read(cx).value().trim().to_owned(),
            team: w.picks.iter().filter(|p| p.on || p.role == lila::roles::PM_ROLE).map(|p| (p.role.clone(), if p.role == lila::roles::PM_ROLE { 1 } else { p.count })).collect(),
        });
        Some(Plan { dir: f.existing.as_ref().map(|e| e.root.clone()).unwrap_or_else(|| f.dir.clone()), git_init: !f.git, new, brief: (!brief.is_empty()).then_some(brief) })
    }

    fn wizard_finish(&mut self, cx: &mut Context<Self>) {
        let Some(plan) = self.wizard_plan(cx) else { return };
        let paths = self.paths().clone();
        if let Some(w) = self.wizard.as_mut() {
            w.busy = true;
            w.error = None;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { run(&paths, plan) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(id) => {
                        this.wizard = None;
                        if !id.is_empty() {
                            this.select_project(id, cx);
                        }
                    }
                    Err(e) => {
                        if let Some(w) = this.wizard.as_mut() {
                            w.busy = false;
                            w.error = Some(format!("{e:#}"));
                        }
                    }
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
    }

    fn wizard_edit(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Wizard)) {
        if let Some(w) = self.wizard.as_mut() {
            edit(w);
            w.error = None;
        }
        cx.notify();
    }

    fn wizard_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Some(w) = self.wizard.as_ref() else { return div().into_any_element() };
        let (title_input, brief) = (w.title.clone(), w.brief.clone());
        let Some(f) = &w.folder else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(Button::new("wizard-pick-big").primary().icon(IconName::FolderOpen).label("选择文件夹").on_click(cx.listener(|this, _, window, cx| this.wizard_pick_folder(window, cx))))
                .into_any_element();
        };
        let change = Button::new("wizard-pick").outline().small().small_button().label("更换").on_click(cx.listener(|this, _, window, cx| this.wizard_pick_folder(window, cx)));
        let mut rows = vec![row("文件夹", None, h_flex().gap_3().child(value_text(tail(&crate::data::tilde(&f.dir.to_string_lossy()), 44), cx)).child(change), cx)];
        let new = f.existing.is_none();
        if new {
            rows.push(row("名称", None, div().w(px(280.)).child(Input::new(&title_input).control()), cx));
        }
        // What to do comes first: it is the one thing every new project needs.
        let mut body = vec![group(None, rows, cx), group(None, vec![block("简介", None, Textarea::new(&brief).area(), None, cx)], cx)];
        // An existing project keeps its team.
        if new {
            let team = w
                .picks
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let (on, count, is_pm) = (p.on, p.count, p.role == lila::roles::PM_ROLE);
                    let missing = p.missing.clone();
                    let check = Checkbox::new(SharedString::from(format!("wizard-role-{}", p.role))).checked(on || is_pm).disabled(is_pm || missing.is_some()).on_click(cx.listener(move |this, on: &bool, _, cx| {
                        let on = *on;
                        this.wizard_edit(cx, |w| w.picks[i].on = on)
                    }));
                    let control = (on && !is_pm).then(|| {
                        let step = move |d: i64| -> Option<Act> {
                            let n = count as i64 + d;
                            (1..=9).contains(&n).then(|| -> Act { Box::new(move |this: &mut LilaApp, cx| this.wizard_edit(cx, |w| w.picks[i].count = n as u32)) })
                        };
                        stepper(&format!("wizard-count-{}", p.role), format!("{count} 个"), step(-1), step(1), cx)
                    });
                    h_flex()
                        .px_4()
                        .h(px(52.))
                        .gap_3()
                        .child(check)
                        .child(div().flex_1().min_w_0().text_size(px(13.)).font_medium().text_color(if on || is_pm { t.foreground } else { t.muted_foreground }).child(p.name.clone()))
                        .children(control)
                        .when_some(missing, |d, cli| d.child(div().text_xs().text_color(t.muted_foreground).child(format!("未安装 {cli}"))))
                        .into_any_element()
                })
                .collect();
            body.push(group(None, team, cx));
        }
        crate::dialog::scroll("wizard", body)
    }

    pub fn wizard_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = self.wizard.as_ref()?;
        let theme = cx.theme().clone();
        let (busy, error) = (w.busy, w.error.clone());
        // A folder whose config doesn't load can't go on; any other failure can be tried again.
        let ready = w.folder.as_ref().is_some_and(|f| match &f.existing {
            Some(e) => e.error.is_none(),
            // A new team needs its PM's CLI.
            None => !w.picks.iter().any(|p| p.role == lila::roles::PM_ROLE && p.missing.is_some()),
        });
        let existing = w.folder.as_ref().is_some_and(|f| f.existing.is_some());
        let body = self.wizard_page(cx);
        let note = error.clone().map(|e| h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element());
        let save_label = match () {
            _ if busy => "正在启动…",
            _ if existing => "启动",
            _ => "创建",
        };
        Some(self.dialog(
            Shell {
                id: "new-project",
                nav: Vec::new(),
                title: "新建项目".into(),
                body,
                note,
                saving: busy,
                save_label,
                can_save: ready,
                on_save: Box::new(|this, cx| this.wizard_finish(cx)),
                on_close: Box::new(|this, _| {
                    if !this.wizard.as_ref().is_some_and(|w| w.busy) {
                        this.wizard = None;
                    }
                }),
                footer: true,
            },
            cx,
        ))
    }
}
