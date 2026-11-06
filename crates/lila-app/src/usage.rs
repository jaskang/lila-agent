//! 用量, a page of the global settings: each CLI's subscription accounts, how
//! much of each usage window is gone and when it resets (`lila::accounts`,
//! read from the CLIs' own records off the UI thread, every 30 s and right
//! after a change). Accounts are added here, signing in in the vendor's web
//! page (`lila::signin`: the CLI's own login in the background, as magpie does),
//! made current (members run on it), or removed.

use crate::app::LilaApp;
use crate::data;
use crate::dialog;
use crate::style;
use crate::style::Control as _;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputState};
use gpui::component::spinner::Spinner;
use gpui::component::{Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::accounts::{self, Account, AccountUsage, Window as UsageWindow};
use lila_core::Runtime;
use std::time::Duration;

#[derive(Default)]
pub struct UsageState {
    /// Every account with what its records say, as last read.
    pub rows: Vec<(Account, AccountUsage)>,
    pub error: Option<String>,
    /// The CLI whose "add account" form is open, and its name field.
    pub adding: Option<(Runtime, Entity<InputState>)>,
    /// An account waiting for a second click to be removed.
    pub confirm_remove: Option<String>,
    /// The account signing in in the browser.
    pub signing: Option<Signing>,
    /// magpie, when it is installed, and the accounts it is signed in to (`lila::magpie`).
    pub magpie_bin: Option<std::path::PathBuf>,
    pub magpie: Vec<lila::magpie::MagpieAccount>,
    /// The magpie account being switched to (its email).
    pub switching: Option<String>,
    /// Reads again now.
    kick: Option<async_channel::Sender<()>>,
}

/// A new account whose login runs: its folder exists, the list has it once it is done.
pub struct Signing {
    pub account: Account,
    signin: lila::signin::SignIn,
    /// The sign-in page, once the login gave it.
    pub url: Option<String>,
    /// A code the page shows when it can't come back to this machine (Claude).
    code: Entity<InputState>,
}

impl UsageState {
    pub fn refresh(&self) {
        if let Some(k) = &self.kick {
            let _ = k.try_send(());
        }
    }
}

/// The CLIs, in the page's order, with the color their meters use.
fn clis() -> [(Runtime, &'static str, Hsla); 2] {
    [(Runtime::Claude, "Claude Code", style::claude()), (Runtime::Codex, "Codex", style::blue())]
}

/// "2 小时 5 分后", "3 天 4 小时后".
fn until(ms: i64, now: i64) -> String {
    let m = ((ms - now).max(0) + 59_999) / 60_000;
    let (d, h) = (m / 1440, m % 1440 / 60);
    match (d, h) {
        (0, 0) => format!("{} 分钟后", m % 60),
        (0, _) => format!("{h} 小时 {} 分后", m % 60),
        (_, 0) => format!("{d} 天后"),
        _ => format!("{d} 天 {h} 小时后"),
    }
}

/// "今天 21:00", "明天 09:00", "11月12日 周四 11:11".
fn when(ms: i64, now: i64) -> String {
    use chrono::Datelike;
    let s = data::local_when(ms, now);
    if data::days_from_today(ms, now).abs() <= 1 {
        return s;
    }
    let week = ["日", "一", "二", "三", "四", "五", "六"][data::local(ms).weekday().num_days_from_sunday() as usize];
    match s.split_once(' ') {
        Some((date, time)) => format!("{date} 周{week} {time}"),
        None => s,
    }
}

/// The fill keeps the CLI's color until the window gets tight, then warns.
fn meter_color(used: f64, cli: Hsla) -> Hsla {
    if used >= 90.0 {
        style::red()
    } else if used >= 75.0 {
        rgb(0xff9f0a).into()
    } else {
        cli
    }
}

/// The PATH a login shell has: an app opened from the Finder gets a bare one,
/// and the CLIs live where the person's shell puts them.
fn login_path() -> String {
    use std::sync::OnceLock;
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::process::Command::new("/bin/zsh").args(["-lc", "printf %s \"$PATH\""]).stdin(std::process::Stdio::null()).output();
        let found = shell.ok().and_then(|o| String::from_utf8(o.stdout).ok()).filter(|p| !p.trim().is_empty());
        let mut p = found.unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
        let home = std::env::var("HOME").unwrap_or_default();
        for extra in [format!("{home}/.local/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into()] {
            if !p.split(':').any(|d| d == extra) {
                p = format!("{p}:{extra}");
            }
        }
        p
    })
    .clone()
}

/// Opens a page in the default browser.
fn open_url(url: &str) {
    // Screenshots sign in without a browser popping up (`LILA_APP_NO_OPEN`).
    if cfg!(feature = "shots") && std::env::var_os("LILA_APP_NO_OPEN").is_some() {
        return;
    }
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Forgets an account that never signed in: its folder goes.
fn discard(a: &Account) {
    if let Some(d) = &a.dir {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Logs an account out (its CLI forgets the login), best effort.
fn logout(a: &Account) {
    let (argv, env) = a.logout_command();
    let mut cmd = std::process::Command::new("/bin/zsh");
    cmd.arg("-lc").arg(argv.join(" ")).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Some((k, v)) = env {
        cmd.env(k, v);
    }
    let _ = cmd.status();
}

impl LilaApp {
    /// Starts the background reads (once, from `LilaApp::new`).
    pub fn start_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kick, kicked) = async_channel::unbounded::<()>();
        self.usage.kick = Some(kick);
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(path) = this.update(cx, |this, _| this.paths().accounts()) else { return };
                let (read, magpie) = cx
                    .background_executor()
                    .spawn(async move {
                        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                        let read = accounts::load(&path).map(|list| list.into_iter().map(|a| {
                            let u = accounts::usage(&a, &home);
                            (a, u)
                        }).collect::<Vec<_>>());
                        // magpie's own reading of its accounts; never their tokens.
                        let magpie = lila::magpie::find(&login_path()).map(|bin| {
                            let list = lila::magpie::accounts(&bin);
                            (bin, list)
                        });
                        (read, magpie)
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        match magpie {
                            Some((bin, Ok(list))) => {
                                this.usage.magpie_bin = Some(bin);
                                this.usage.magpie = list;
                            }
                            Some((bin, Err(e))) => {
                                // Keep what was read last; say why it is not fresh.
                                this.usage.magpie_bin = Some(bin);
                                this.usage.error = Some(format!("读取 magpie 的账号失败：{e:#}"));
                            }
                            None => {
                                this.usage.magpie_bin = None;
                                this.usage.magpie.clear();
                            }
                        }
                        match read {
                            Ok(rows) => {
                                this.usage.rows = rows;
                                this.usage.error = None;
                            }
                            Err(e) => this.usage.error = Some(format!("读取账号失败：{e:#}")),
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                let asked = std::pin::pin!(kicked.recv());
                let timer = std::pin::pin!(cx.background_executor().timer(Duration::from_secs(30)));
                futures::future::select(asked, timer).await;
                while kicked.try_recv().is_ok() {}
            }
        })
        .detach();
    }

    /// `LILA_APP_SIGNIN=claude:<name>` (screenshots): the usage page, adding that account.
    #[cfg(feature = "shots")]
    pub fn dev_signin(&mut self, spec: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (cli, name) = spec.split_once(':').unwrap_or((spec, ""));
        let r = if cli == "codex" { Runtime::Codex } else { Runtime::Claude };
        self.open_prefs(crate::prefs::PrefPage::Usage, window, cx);
        self.account_add(r, window, cx);
        if let Some((_, input)) = &self.usage.adding {
            input.update(cx, |i, cx| i.set_value(name.to_owned(), window, cx));
        }
        self.account_login(window, cx);
    }

    fn account_add(&mut self, r: Runtime, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("账号名称，例如 工作、备用"));
        input.update(cx, |i, cx| i.focus(window, cx));
        self.usage.adding = Some((r, input));
        self.usage.confirm_remove = None;
        cx.notify();
    }

    /// Starts the CLI's own login for a new account; its page opens in the browser.
    fn account_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((r, input)) = self.usage.adding.clone() else { return };
        if self.usage.signing.is_some() {
            return;
        }
        let name = input.read(cx).value().to_string();
        let paths = self.paths().clone();
        let path = login_path();
        let cli = if r == Runtime::Claude { "claude" } else { "codex" };
        let started = (|| {
            let program = lila::paths::which(cli, &path).ok_or_else(|| anyhow::anyhow!("找不到 `{cli}` 命令：先安装它，并确认登录 shell 的 PATH 里有它"))?;
            let a = accounts::prepare(&paths.accounts(), &paths.account_dirs(), r, &name)?;
            match lila::signin::start(&a, &program, &path) {
                Ok(s) => Ok((a, s)),
                Err(e) => {
                    discard(&a);
                    Err(e)
                }
            }
        })();
        match started {
            Ok((account, signin)) => {
                let code = cx.new(|cx| InputState::new(window, cx).placeholder("网页上显示了一段代码？贴在这里"));
                self.usage.signing = Some(Signing { account, signin, url: None, code });
                self.usage.error = None;
                self.watch_signin(cx);
            }
            Err(e) => self.usage.error = Some(format!("没能开始登录：{e:#}")),
        }
        cx.notify();
    }

    /// Follows the login until it ends.
    fn watch_signin(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let go_on = this.update(cx, |this, cx| this.signin_events(cx)).unwrap_or(false);
                if !go_on {
                    return;
                }
            }
        })
        .detach();
    }

    /// What the login said since: the page to open, then how it ended. False once it has.
    fn signin_events(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.usage.signing.as_mut() else { return false };
        let mut ended: Option<Result<(), String>> = None;
        while let Ok(ev) = s.signin.events.try_recv() {
            match ev {
                lila::signin::Event::Link { url, opened } => {
                    if !opened {
                        open_url(&url);
                    }
                    s.url = Some(url);
                    cx.notify();
                }
                lila::signin::Event::Done => ended = Some(Ok(())),
                lila::signin::Event::Failed(why) => ended = Some(Err(why)),
            }
        }
        let Some(end) = ended else { return true };
        let s = self.usage.signing.take().expect("checked");
        match end {
            Ok(()) => match accounts::keep(&self.paths().accounts(), &s.account) {
                Ok(()) => {
                    self.usage.adding = None;
                    self.usage.error = None;
                    self.notice = Some(format!("已添加账号「{}」。设为当前后，成员重启时会用它。", s.account.name));
                }
                Err(e) => self.usage.error = Some(format!("登录成功，但没能保存账号：{e:#}")),
            },
            Err(why) => {
                discard(&s.account);
                self.usage.error = Some(format!("登录没有完成：{why}"));
            }
        }
        self.usage.refresh();
        cx.notify();
        false
    }

    fn signin_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.usage.signing.take() {
            s.signin.cancel();
            discard(&s.account);
        }
        self.usage.adding = None;
        cx.notify();
    }

    fn signin_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.usage.signing.as_ref() else { return };
        let code = s.code.read(cx).value().to_string();
        if code.trim().is_empty() {
            return;
        }
        if let Err(e) = s.signin.paste(&code) {
            self.usage.error = Some(format!("没能交给登录：{e:#}"));
        }
        s.code.update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    fn account_use(&mut self, id: String, cx: &mut Context<Self>) {
        if let Err(e) = accounts::set_current(&self.paths().accounts(), &id) {
            self.usage.error = Some(format!("切换账号失败：{e:#}"));
        }
        self.usage.confirm_remove = None;
        self.usage.refresh();
        cx.notify();
    }

    /// The first click asks; the second logs out and deletes the account's folder.
    fn account_remove(&mut self, id: String, cx: &mut Context<Self>) {
        if self.usage.confirm_remove.as_deref() != Some(&id) {
            self.usage.confirm_remove = Some(id);
            cx.notify();
            return;
        }
        self.usage.confirm_remove = None;
        let path = self.paths().accounts();
        let Some(account) = self.usage.rows.iter().map(|(a, _)| a).find(|a| a.id == id).cloned() else { return };
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    logout(&account);
                    accounts::remove(&path, &account.id)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = done {
                    this.usage.error = Some(format!("移除账号失败：{e:#}"));
                }
                this.usage.refresh();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn usage_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let now = data::now_ms();
        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(e) = self.usage.error.clone() {
            body.push(h_flex().gap_2().text_size(px(12.)).text_color(style::red()).child(Icon::new(IconName::CircleAlert).small()).child(e).into_any_element());
        }
        for (r, label, color) in clis() {
            let mut list: Vec<(Account, AccountUsage)> = self.usage.rows.iter().filter(|(a, _)| a.runtime == r).cloned().collect();
            let from_magpie: Vec<lila::magpie::MagpieAccount> = self.usage.magpie.iter().filter(|m| m.runtime == r).cloned().collect();
            // magpie's accounts are the CLI's own folder's sign-ins: they stand in for Lila's default account.
            let own_current = list.iter().any(|(a, _)| a.is_default() && a.current);
            if !from_magpie.is_empty() {
                list.retain(|(a, _)| !a.is_default());
            }
            let count = list.len() + from_magpie.len();
            let adding = self.usage.adding.as_ref().filter(|(cli, _)| *cli == r).map(|(_, i)| i.clone());
            let head = h_flex()
                .px_1()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(style::muted2())
                .child(div().font_semibold().child(label))
                .child(div().child(format!("{count} 个账号")))
                .child(div().flex_1())
                .when(adding.is_none(), |d| {
                    d.child(style::link(SharedString::from(format!("acct-add-{r}")), "添加账号", true).text_size(px(11.)).on_click(cx.listener(move |this, _, window, cx| this.account_add(r, window, cx))))
                });
            let mut cards: Vec<AnyElement> = from_magpie.iter().map(|m| self.magpie_card(m, own_current, color, now, cx)).collect();
            cards.extend(list.iter().map(|(a, u)| self.account_card(a, u, color, now, cx)));
            if let Some(input) = adding {
                cards.push(self.add_form(label, color, input, cx));
            }
            body.push(v_flex().gap_2().child(head).child(v_flex().gap_2().children(cards)).into_any_element());
        }
        body.push(div().text_xs().line_height(relative(1.6)).text_color(style::muted()).child("成员都用「正在使用」的账号。切换后，已经在跑的成员下次重启时才换过去。").into_any_element());
        if !self.usage.magpie.is_empty() {
            body.push(
                div()
                    .text_xs()
                    .line_height(relative(1.6))
                    .text_color(style::muted())
                    .child("标着 magpie 的账号由 magpie 管理登录，Lila 不复制它们的令牌。设为当前时由 magpie 切换 CLI 自己的登录，你在终端里开的 claude / codex 也会一起换。")
                    .into_any_element(),
            );
        }
        dialog::scroll("prefs-usage", body)
    }

    /// Members on the CLI's own folder, and that folder signed in to `user` by magpie.
    fn magpie_use(&mut self, r: Runtime, user: String, active: bool, cx: &mut Context<Self>) {
        let Some(bin) = self.usage.magpie_bin.clone() else { return };
        let path = self.paths().accounts();
        let own = self.usage.rows.iter().map(|(a, _)| a).find(|a| a.runtime == r && a.is_default()).map(|a| a.id.clone()).unwrap_or_else(|| format!("{r}-default"));
        self.usage.switching = Some(user.clone());
        self.usage.confirm_remove = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    if !active {
                        lila::magpie::switch(&bin, r, &user)?;
                    }
                    accounts::set_current(&path, &own)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.usage.switching = None;
                if let Err(e) = done {
                    this.usage.error = Some(format!("切换账号失败：{e:#}"));
                }
                this.usage.refresh();
                cx.notify();
            });
        })
        .detach();
    }

    /// An account magpie signed in to: its own reading of the windows, picked through magpie.
    fn magpie_card(&mut self, m: &lila::magpie::MagpieAccount, own_current: bool, color: Hsla, now: i64, cx: &mut Context<Self>) -> AnyElement {
        let u = &m.usage;
        let seen = match u.seen_ms {
            Some(ms) => format!("{} 更新", data::clock(ms)),
            None => "还没有记录".into(),
        };
        let switching = self.usage.switching.as_deref() == Some(&m.user);
        let actions = if m.active && own_current {
            style::tag(style::green(), "正在使用").into_any_element()
        } else if switching {
            div().text_size(px(13.)).text_color(style::muted()).child("切换中…").into_any_element()
        } else {
            let (r, user, active) = (m.runtime, m.user.clone(), m.active);
            let id = SharedString::from(format!("mg-use-{}-{}", m.runtime, m.user));
            style::link(id, "设为当前", false).text_size(px(13.)).on_click(cx.listener(move |this, _, _, cx| this.magpie_use(r, user.clone(), active, cx))).into_any_element()
        };
        let head = h_flex()
            .px_4()
            .py_3()
            .min_h(px(58.))
            .gap_4()
            .child(tile(IconName::User, color))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_size(px(13.)).font_medium().truncate().child(m.user.clone()))
                            .when_some(u.plan.clone(), |d, p| d.child(div().text_xs().text_color(style::muted()).child(p)))
                            .child(style::tag_neutral("magpie")),
                    )
                    .child(div().text_xs().text_color(style::muted2()).child(seen)),
            )
            .child(actions);
        let mut rows = vec![head.into_any_element()];
        rows.extend(u.windows.iter().map(|w| window_row(w, color, now)));
        let n = rows.len();
        v_flex()
            .rounded(px(10.))
            .border_1()
            .border_color(style::hairline())
            .bg(dialog::card(cx))
            .children(rows.into_iter().enumerate().map(|(i, r)| div().when(i + 1 < n, |d| d.border_b_1().border_color(Hsla::from(rgba(0xffffff0d)))).child(r)))
            .into_any_element()
    }

    fn account_card(&mut self, a: &Account, u: &AccountUsage, color: Hsla, now: i64, cx: &mut Context<Self>) -> AnyElement {
        let id = a.id.clone();
        let confirm = self.usage.confirm_remove.as_deref() == Some(&a.id);
        let seen = match u.seen_ms {
            Some(ms) if !u.windows.is_empty() => format!("{} 更新", data::clock(ms)),
            _ => "还没有记录".into(),
        };
        let actions = if a.current {
            style::tag(style::green(), "正在使用").into_any_element()
        } else {
            let (use_id, rm_id) = (id.clone(), id.clone());
            h_flex()
                .gap(px(10.))
                .child(style::link(SharedString::from(format!("acct-use-{id}")), "设为当前", false).text_size(px(13.)).on_click(cx.listener(move |this, _, _, cx| this.account_use(use_id.clone(), cx))))
                .when(!a.is_default(), |d| {
                    let remove = if confirm {
                        Button::new(SharedString::from(format!("acct-rm-{id}"))).small().danger().label("确认移除")
                    } else {
                        Button::new(SharedString::from(format!("acct-rm-{id}"))).ghost().xsmall().icon(style::icon("lila/trash.svg")).tooltip("移除这个账号（退出登录并删除它的配置目录）")
                    };
                    d.child(remove.on_click(cx.listener(move |this, _, _, cx| this.account_remove(rm_id.clone(), cx))))
                })
                .into_any_element()
        };
        let head = h_flex()
            .px_4()
            .py_3()
            .min_h(px(58.))
            .gap_4()
            .child(tile(IconName::User, color))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(h_flex().gap_2().child(div().text_size(px(13.)).font_medium().child(a.name.clone())).when_some(u.plan.clone(), |d, p| d.child(div().text_xs().text_color(style::muted()).child(p))))
                    .child(div().text_xs().text_color(style::muted2()).child(seen)),
            )
            .child(actions);
        let mut rows = vec![head.into_any_element()];
        if u.windows.is_empty() {
            rows.push(h_flex().px_4().py_3().text_xs().text_color(style::muted()).child("有成员用它工作后才有数据。").into_any_element());
        }
        rows.extend(u.windows.iter().map(|w| window_row(w, color, now)));
        let n = rows.len();
        v_flex()
            .rounded(px(10.))
            .border_1()
            .border_color(style::hairline())
            .bg(dialog::card(cx))
            .children(rows.into_iter().enumerate().map(|(i, r)| div().when(i + 1 < n, |d| d.border_b_1().border_color(Hsla::from(rgba(0xffffff0d)))).child(r)))
            .into_any_element()
    }

    fn add_form(&mut self, label: &'static str, color: Hsla, input: Entity<InputState>, cx: &mut Context<Self>) -> AnyElement {
        if let Some(s) = self.usage.signing.as_ref().filter(|s| s.account.runtime == if label == "Claude Code" { Runtime::Claude } else { Runtime::Codex }) {
            let (url, code, claude) = (s.url.clone(), s.code.clone(), s.account.runtime == Runtime::Claude);
            return v_flex()
                .rounded(px(10.))
                .border_1()
                .border_color(style::hairline())
                .bg(dialog::card(cx))
                .child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap(px(10.))
                        .child(div().size(px(30.)).flex_none().rounded(px(8.)).flex().items_center().justify_center().bg(color.opacity(0.2)).border_1().border_color(color.opacity(0.28)).child(Spinner::new().small().color(style::tint(color, 0.4))))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(1.))
                                .child(div().text_size(px(13.)).font_medium().child(format!("正在浏览器里登录 {label}…")))
                                .child(div().text_xs().text_color(style::muted2()).truncate().child(format!("「{}」 · 在网页上登录后会自动回到这里", s.account.name))),
                        )
                        .when_some(url, |d, url| d.child(style::link("acct-reopen", "重新打开登录页", false).text_size(px(13.)).on_click(move |_, _, _| open_url(&url))))
                        .child(Button::new("acct-cancel").ghost().label("取消").on_click(cx.listener(|this, _, _, cx| this.signin_cancel(cx)))),
                )
                .when(claude, |d| {
                    d.child(
                        h_flex()
                            .px_4()
                            .pb_3()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&code).control()))
                            .child(Button::new("acct-code").ghost().label("提交").on_click(cx.listener(|this, _, window, cx| this.signin_code(window, cx)))),
                    )
                })
                .into_any_element();
        }
        v_flex()
            .rounded(px(10.))
            .border_1()
            .border_color(style::hairline())
            .bg(dialog::card(cx))
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap(px(10.))
                    .child(tile(IconName::Plus, color))
                    .child(div().flex_1().child(Input::new(&input).control()))
                    .child(Button::new("acct-cancel").ghost().label("取消").on_click(cx.listener(|this, _, _, cx| this.signin_cancel(cx))))
                    .child(Button::new("acct-login").primary().label("登录…").on_click(cx.listener(|this, _, window, cx| this.account_login(window, cx)))),
            )
            .child(div().px_4().pb_3().text_xs().text_color(style::muted()).child(format!("会在浏览器里打开 {} 的登录页。", if label == "Claude Code" { "Claude" } else { "ChatGPT" })))
            .into_any_element()
    }
}

/// The 30 px account tile: an icon in the CLI's color on a soft wash.
fn tile(icon: IconName, color: Hsla) -> Div {
    div()
        .size(px(30.))
        .flex_none()
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .bg(color.opacity(0.2))
        .border_1()
        .border_color(color.opacity(0.28))
        .child(Icon::new(icon).small().text_color(style::tint(color, 0.4)))
}

/// One window: its name, how much is used, when it resets, and a meter.
fn window_row(w: &UsageWindow, color: Hsla, now: i64) -> AnyElement {
    let used = w.used_percent.clamp(0.0, 100.0);
    h_flex()
        .px_4()
        .py_3()
        .min_h(px(58.))
        .gap_4()
        .child(div().w(px(72.)).flex_none().text_size(px(13.)).font_medium().child(w.name.clone()))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(7.))
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_size(px(13.)).font_medium().text_color(if used >= 90.0 { rgb(0xff6961).into() } else { style::fg() }).child(format!("已用 {}%", used.round() as i64)))
                        .child(div().flex_1())
                        .child(div().text_xs().text_color(style::muted()).child(format!("{}重置 · {}", until(w.resets_ms, now), when(w.resets_ms, now)))),
                )
                .child(
                    div()
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(Hsla::from(rgba(0xffffff14)))
                        .overflow_hidden()
                        .child(div().h_full().rounded(px(3.)).w(relative((used / 100.0) as f32)).bg(meter_color(used, color))),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` macro would stand in for `#[test]`.
    use super::{until, when};

    #[test]
    fn reset_times_read_like_the_prototype() {
        let now = 1_000_000_000_000;
        assert_eq!(until(now + 30 * 60_000, now), "30 分钟后");
        assert_eq!(until(now + 125 * 60_000, now), "2 小时 5 分后");
        assert_eq!(until(now + 3 * 86_400_000, now), "3 天后");
        assert_eq!(until(now + (3 * 24 + 4) * 3_600_000, now), "3 天 4 小时后");
        assert!(when(now + 5 * 86_400_000, now).contains(" 周"), "{}", when(now + 5 * 86_400_000, now));
        assert!(!when(now, now).contains('周'));
    }
}
