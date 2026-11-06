//! The new-project wizard, a dialog in three steps: the folder (and the
//! project's id and name), the team (roles and how many of each), then the
//! brief the PM gets as its first task. Finishing writes `.lila/project.toml`
//! (unless the folder already is a project) and starts the team.

use crate::app::LilaApp;
use crate::dialog::{Act, Nav, Shell, block, group, row, stepper, value_text};
use crate::settings::notice;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::checkbox::Checkbox;
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::switch::Switch;
use gpui::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use lila::init::NewProject;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Folder,
    Team,
    Brief,
}

impl Step {
    /// For `LILA_APP_NEW_STEP=<step>` (screenshots).
    #[cfg(feature = "shots")]
    pub fn parse(s: &str) -> Step {
        match s {
            "team" => Step::Team,
            "brief" => Step::Brief,
            _ => Step::Folder,
        }
    }
}

/// A role the team can have.
pub struct Pick {
    pub role: String,
    pub name: String,
    /// "claude-opus-5-5 · high".
    pub about: String,
    pub on: bool,
    pub count: u32,
}

/// A folder that already is (or is inside) a Lila project: its team is kept.
pub struct Existing {
    pub root: PathBuf,
    pub name: String,
    /// "pm · PM（项目经理）" per member, or why the config doesn't load.
    pub members: Result<Vec<(String, String)>, String>,
}

pub struct Folder {
    pub dir: PathBuf,
    pub existing: Option<Existing>,
    /// The id the database has for this folder (its config was removed).
    pub registered: Option<String>,
    pub git: bool,
}

pub struct Wizard {
    pub step: Step,
    pub folder: Option<Folder>,
    pub id: Entity<InputState>,
    pub title: Entity<InputState>,
    /// Run `git init` first (offered when the folder isn't a repository).
    pub git_init: bool,
    pub picks: Vec<Pick>,
    pub brief: Entity<TextareaState>,
    pub error: Option<String>,
    pub busy: bool,
}

/// What finishing does, worked out on the UI thread and run off it.
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
    match &plan.new {
        Some(p) => {
            lila::init::create_project(paths, &plan.dir, p)?;
        }
        None => {
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
    /// Opens the wizard on its first step (no folder yet).
    pub fn new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.paths().clone();
        // A fresh machine has no roles yet: put the defaults in place to choose from.
        let _ = lila::roles::install_defaults(&paths.roles());
        let roles = lila::roles::load(&paths.roles()).unwrap_or_default();
        let default_team = ["pm", "designer", "architect", "engineer", "tester"];
        let order = |id: &str| lila::roles::DEFAULTS.iter().position(|(d, _, _)| *d == id).unwrap_or(usize::MAX);
        let mut roles: Vec<_> = roles.into_values().collect();
        roles.sort_by(|a, b| (order(&a.id), &a.id).cmp(&(order(&b.id), &b.id)));
        let picks = roles
            .iter()
            .map(|r| {
                let f = &r.file;
                let about = match &f.effort {
                    Some(e) => format!("{} · {e}", f.model),
                    None => f.model.clone(),
                };
                Pick { role: r.id.clone(), name: f.name.clone(), about, on: default_team.contains(&r.id.as_str()), count: 1 }
            })
            .collect();
        self.settings = None;
        self.prefs = None;
        self.draft = None;
        self.dialog_offset = Point::default();
        self.wizard = Some(Wizard {
            step: Step::Folder,
            folder: None,
            id: cx.new(|cx| InputState::new(window, cx).placeholder("my-project")),
            title: cx.new(|cx| InputState::new(window, cx).placeholder("可以用中文")),
            git_init: true,
            picks,
            brief: cx.new(|cx| {
                TextareaState::new(window, cx).rows(7).placeholder("想做什么？比如：\n\n做一个记账小程序\n用户可以记录每天的收入和支出，按月看汇总图表。先做网页版，数据存在本地。")
            }),
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
        let registered = self.snap.projects.iter().find(|p| p.cwd == dir_s).map(|p| p.id.clone());
        let existing = lila::paths::find_project_root(&dir).map(|root| {
            let members = lila::settings::load(&root, &roles).map(|s| s.members.iter().flat_map(|m| m.ids().into_iter().map(|id| (id, m.role_name.clone()))).collect()).map_err(|e| format!("{e:#}"));
            let name = lila::settings::load(&root, &roles).map(|s| s.name).unwrap_or_default();
            Existing { root, name, members }
        });
        let folder_name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // A name with nothing ASCII in it (记账本) makes no useful id: leave it to type.
        let id = match &registered {
            Some(id) => id.clone(),
            None if folder_name.chars().any(|c| c.is_ascii_alphanumeric()) => lila::init::slug(&folder_name),
            None => String::new(),
        };
        let Some(w) = self.wizard.as_mut() else { return };
        w.id.update(cx, |s, cx| s.set_value(id, window, cx));
        w.title.update(cx, |s, cx| s.set_value(folder_name, window, cx));
        w.git_init = true;
        w.error = None;
        w.folder = Some(Folder { git: is_git(&dir), dir, existing, registered });
        cx.notify();
    }

    /// Why the wizard can't go past the folder step yet, if it can't.
    fn wizard_folder_problem(&self, cx: &App) -> Option<String> {
        let w = self.wizard.as_ref()?;
        let Some(f) = &w.folder else { return Some("先选一个文件夹".into()) };
        if let Some(e) = &f.existing {
            return e.members.as_ref().err().map(|e| format!("读不了这个项目的配置：{e}"));
        }
        let id = w.id.read(cx).value().trim().to_owned();
        if id.is_empty() {
            return Some("填一个项目编号".into());
        }
        if let Err(e) = lila_core::config::validate_id(&id) {
            return Some(format!("项目编号 {id:?} 不行：{e}"));
        }
        let dir = f.dir.to_string_lossy();
        if let Some(p) = self.snap.projects.iter().find(|p| p.id == id && p.cwd != dir) {
            return Some(format!("{id} 已经是 {} 的编号，换一个", crate::data::tilde(&p.cwd)));
        }
        None
    }

    pub fn wizard_go(&mut self, step: Step, cx: &mut Context<Self>) {
        let problem = if step > Step::Folder { self.wizard_folder_problem(cx) } else { None };
        let Some(w) = self.wizard.as_mut() else { return };
        match problem {
            Some(p) if w.folder.is_some() => w.error = Some(p),
            Some(_) => {}
            None => {
                w.error = None;
                w.step = step;
            }
        }
        cx.notify();
    }

    fn wizard_next(&mut self, cx: &mut Context<Self>) {
        let Some(step) = self.wizard.as_ref().map(|w| w.step) else { return };
        match step {
            Step::Folder => self.wizard_go(Step::Team, cx),
            Step::Team => self.wizard_go(Step::Brief, cx),
            Step::Brief => self.wizard_finish(cx),
        }
    }

    fn wizard_plan(&self, cx: &App) -> Option<Plan> {
        let w = self.wizard.as_ref()?;
        let f = w.folder.as_ref()?;
        let brief = w.brief.read(cx).value().trim().to_owned();
        let new = f.existing.is_none().then(|| NewProject {
            name: w.id.read(cx).value().trim().to_owned(),
            title: w.title.read(cx).value().trim().to_owned(),
            team: w.picks.iter().filter(|p| p.on || p.role == lila::roles::PM_ROLE).map(|p| (p.role.clone(), if p.role == lila::roles::PM_ROLE { 1 } else { p.count })).collect(),
        });
        Some(Plan { dir: f.existing.as_ref().map(|e| e.root.clone()).unwrap_or_else(|| f.dir.clone()), git_init: !f.git && w.git_init, new, brief: (!brief.is_empty()).then_some(brief) })
    }

    fn wizard_finish(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.wizard_folder_problem(cx) {
            if let Some(w) = self.wizard.as_mut() {
                w.error = Some(p);
                w.step = Step::Folder;
            }
            cx.notify();
            return;
        }
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

    fn folder_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Some(w) = self.wizard.as_ref() else { return div().into_any_element() };
        let (id_input, title_input, git_init) = (w.id.clone(), w.title.clone(), w.git_init);
        let Some(f) = &w.folder else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .px_12()
                .child(Icon::new(IconName::FolderOpen).large().text_color(t.muted_foreground))
                .child(div().text_size(px(13.)).text_center().text_color(t.muted_foreground).child("选一个代码仓库，或者一个空文件夹。团队的成员会在这个文件夹里读写文件。"))
                .child(Button::new("wizard-pick-big").primary().label("选择文件夹…").on_click(cx.listener(|this, _, window, cx| this.wizard_pick_folder(window, cx))))
                .into_any_element();
        };
        let change = Button::new("wizard-pick").outline().small().small_button().label("更换…").on_click(cx.listener(|this, _, window, cx| this.wizard_pick_folder(window, cx)));
        let mut body = vec![group(None, vec![row("文件夹", None, h_flex().gap_3().child(value_text(tail(&crate::data::tilde(&f.dir.to_string_lossy()), 44), cx)).child(change), cx)], cx)];
        match &f.existing {
            Some(e) => {
                let text = if e.root == f.dir {
                    format!("这个文件夹已经是 Lila 项目「{}」，会沿用它 .lila/project.toml 里的团队。", e.name)
                } else {
                    format!("这个文件夹在 {} 的项目「{}」里面，会打开那个项目，沿用它的团队。", crate::data::tilde(&e.root.to_string_lossy()), e.name)
                };
                body.push(notice(IconName::CircleCheck, t.green, &text, cx));
            }
            None => {
                let id_row = match &f.registered {
                    Some(id) => row("项目编号", Some("这个文件夹以前登记过，编号沿用原来的。".into()), value_text(id.clone(), cx), cx),
                    None => row("项目编号", Some("用在终端名和命令里：小写字母、数字和 -。以后不能改。".into()), div().w(px(280.)).child(Input::new(&id_input).control()), cx),
                };
                let title_row = row("显示名称", Some("侧栏和标签上显示的名字，以后可以在项目设置里改。".into()), div().w(px(280.)).child(Input::new(&title_input).control()), cx);
                body.push(group(Some("项目"), vec![id_row, title_row], cx));
                if f.registered.is_some() {
                    body.push(notice(IconName::Info, t.muted_foreground, "它的 .lila/project.toml 不在了，会按下一步选的团队重新创建。以前的任务和动态还在。", cx));
                }
            }
        }
        if !f.git {
            let switch = Switch::new("wizard-git").checked(git_init).on_click(cx.listener(|this, on: &bool, _, cx| {
                let on = *on;
                this.wizard_edit(cx, |w| w.git_init = on)
            }));
            body.push(group(Some("版本管理"), vec![row("初始化 git 仓库", Some("这个文件夹还不是 git 仓库。成员会在这里改文件，用 git 才看得清改了什么、能撤回。".into()), switch, cx)], cx));
        }
        crate::dialog::scroll("wizard-folder", body)
    }

    fn team_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Some(w) = self.wizard.as_ref() else { return div().into_any_element() };
        if let Some(e) = w.folder.as_ref().and_then(|f| f.existing.as_ref()) {
            let rows = match &e.members {
                Ok(m) => m.iter().map(|(id, role)| row(id.clone(), None, value_text(role.clone(), cx), cx)).collect(),
                Err(err) => vec![row("读不了配置", Some(err.clone()), div(), cx)],
            };
            return crate::dialog::scroll(
                "wizard-team",
                vec![group(Some("沿用的团队"), rows, cx), div().text_xs().text_color(t.muted_foreground).child("要增减成员，之后在项目设置里改。").into_any_element()],
            );
        }
        let picks: Vec<(usize, String, String, String, bool, u32)> = w.picks.iter().enumerate().map(|(i, p)| (i, p.role.clone(), p.name.clone(), p.about.clone(), p.on, p.count)).collect();
        let total: u32 = w.picks.iter().filter(|p| p.on || p.role == lila::roles::PM_ROLE).map(|p| if p.role == lila::roles::PM_ROLE { 1 } else { p.count }).sum();
        let rows = picks
            .into_iter()
            .map(|(i, role, name, about, on, count)| {
                let is_pm = role == lila::roles::PM_ROLE;
                let check = Checkbox::new(SharedString::from(format!("wizard-role-{role}"))).checked(on || is_pm).disabled(is_pm).on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.wizard_edit(cx, |w| w.picks[i].on = on)
                }));
                let about = if is_pm { format!("{about} · 每个项目一个，负责拆任务、分派、向你汇报") } else { about };
                let control = if is_pm || !on {
                    value_text(if is_pm { "1 个" } else { "" }, cx).into_any_element()
                } else {
                    let step = move |d: i64| -> Option<Act> {
                        let n = count as i64 + d;
                        (1..=9).contains(&n).then(|| -> Act { Box::new(move |this: &mut LilaApp, cx| this.wizard_edit(cx, |w| w.picks[i].count = n as u32)) })
                    };
                    stepper(&format!("wizard-count-{role}"), format!("{count} 个"), step(-1), step(1), cx)
                };
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(check)
                    .child(v_flex().flex_1().min_w_0().gap_0p5().child(h_flex().gap_2().child(div().text_size(px(13.)).font_medium().child(name)).child(div().text_xs().text_color(t.muted_foreground).child(role))).child(div().text_xs().text_color(t.muted_foreground).child(about)))
                    .child(div().flex_none().child(control))
                    .into_any_element()
            })
            .collect();
        crate::dialog::scroll(
            "wizard-team",
            vec![
                group(Some("角色"), rows, cx),
                div()
                    .text_xs()
                    .text_color(t.muted_foreground)
                    .child(format!("共 {total} 个成员，每个都开着一个 CLI，用的是你的订阅额度。谁能派活给谁先按角色默认，之后可以在项目设置里改。角色本身在「设置 → 角色」里管理。"))
                    .into_any_element(),
            ],
        )
    }

    fn brief_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Some(w) = self.wizard.as_ref() else { return div().into_any_element() };
        let brief = w.brief.clone();
        let mut will = Vec::new();
        if let Some(plan) = self.wizard_plan(cx) {
            if plan.git_init {
                will.push("在文件夹里运行 git init".to_owned());
            }
            match &plan.new {
                Some(p) => {
                    let n: u32 = p.team.iter().map(|(_, c)| c).sum();
                    will.push("写入 .lila/project.toml（随仓库提交，任务文件不提交）".to_owned());
                    will.push(format!("启动 {n} 个成员，各自在终端里打开 claude 或 codex"));
                }
                None => will.push("启动团队里还没运行的成员".to_owned()),
            }
            will.push(if plan.brief.is_some() { "把简介作为第一个任务交给 PM，它会拆分、分派".to_owned() } else { "不写简介：之后直接在 PM 的终端里说要做什么".to_owned() });
        }
        let will_rows = will.into_iter().map(|s| h_flex().px_4().py_2p5().gap_2().text_size(px(13.)).child(Icon::new(IconName::Check).small().text_color(t.muted_foreground)).child(s).into_any_element()).collect();
        crate::dialog::scroll(
            "wizard-brief",
            vec![
                group(None, vec![block("项目简介", Some("第一行作为任务标题，其余是说明，支持 Markdown。写清楚要做什么、给谁用、做到什么程度算完成。".into()), Textarea::new(&brief).area(), None, cx)], cx),
                group(Some("接下来会"), will_rows, cx),
            ],
        )
    }

    pub fn wizard_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let w = self.wizard.as_ref()?;
        let theme = cx.theme().clone();
        let (step, busy, error, has_folder) = (w.step, w.busy, w.error.clone(), w.folder.is_some());
        let existing = w.folder.as_ref().is_some_and(|f| f.existing.is_some());
        let (title, description, body) = match step {
            Step::Folder => ("选择文件夹", "项目放在哪里。可以是已有的代码仓库，也可以是空文件夹。", self.folder_page(cx)),
            Step::Team => ("团队", if existing { "这个项目已经有团队了。" } else { "选要哪些角色，每个角色开几个。PM 必须有。" }, self.team_page(cx)),
            Step::Brief => ("项目简介", "告诉 PM 要做什么。它会读完、拆成任务、分派给团队，遇到要你决定的会问你。", self.brief_page(cx)),
        };
        let note = error.map(|e| h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element());
        let go = |s: Step| -> Act { Box::new(move |this: &mut LilaApp, cx| this.wizard_go(s, cx)) };
        let mark = |s: Step| if s < step { Some("✓".to_owned()) } else { None };
        let nav = vec![
            (Nav { id: "wizard-nav-folder", icon: IconName::FolderOpen, label: "1  文件夹", active: step == Step::Folder, badge: mark(Step::Folder) }, go(Step::Folder)),
            (Nav { id: "wizard-nav-team", icon: IconName::Bot, label: "2  团队", active: step == Step::Team, badge: mark(Step::Team) }, go(Step::Team)),
            (Nav { id: "wizard-nav-brief", icon: IconName::FileText, label: "3  简介", active: step == Step::Brief, badge: None }, go(Step::Brief)),
        ];
        let heading = v_flex().gap_1().child(div().text_xs().text_color(theme.muted_foreground).child("新建项目")).child(div().font_semibold().child("三步开始一个团队")).into_any_element();
        let save_label = match step {
            Step::Brief if busy => "正在启动…",
            Step::Brief if existing => "启动团队",
            Step::Brief => "创建并启动",
            _ => "下一步",
        };
        let back: Option<Act> = match step {
            Step::Folder => None,
            Step::Team => Some(go(Step::Folder)),
            Step::Brief => Some(go(Step::Team)),
        };
        Some(self.dialog(
            Shell {
                id: "new-project",
                heading,
                nav,
                title: title.into(),
                description: description.into(),
                body,
                note,
                saving: busy,
                save_label,
                can_save: has_folder,
                back,
                on_save: Box::new(|this, cx| this.wizard_next(cx)),
                on_close: Box::new(|this, _| {
                    if !this.wizard.as_ref().is_some_and(|w| w.busy) {
                        this.wizard = None;
                    }
                }),
            },
            cx,
        ))
    }
}
