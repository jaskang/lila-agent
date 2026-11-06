//! The 用量 section of the right panel: tokens per member over today, the
//! last 7 days or everything, and how much of Codex's usage window is gone.
//! Read from the CLIs' own session records by `lila::usage`, off the UI thread,
//! every 30 s and whenever the project or the period changes.

use crate::app::LilaApp;
use crate::data;
use crate::ui::s;
use gpui::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::usage::{Limit, Scanner, Sources, Usage, short};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    #[default]
    Today,
    Week,
    All,
}

impl Period {
    const ALL: [Period; 3] = [Period::Today, Period::Week, Period::All];

    /// For `LILA_APP_USAGE=today|week|all` (screenshots).
    #[cfg(feature = "shots")]
    pub fn parse(s: &str) -> Period {
        match s {
            "week" => Period::Week,
            "all" => Period::All,
            _ => Period::Today,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Period::Today => "今天",
            Period::Week => "7 天",
            Period::All => "全部",
        }
    }

    fn since(self, now: i64) -> Option<i64> {
        match self {
            Period::Today => Some(data::local_midnight_ms(now)),
            Period::Week => Some(now - 7 * 86_400_000),
            Period::All => None,
        }
    }
}

/// What the section shows, and for which project and period it was read.
pub struct Shown {
    pub project: String,
    pub period: Period,
    pub usage: Usage,
}

#[derive(Default)]
pub struct UsageState {
    pub period: Period,
    pub shown: Option<Shown>,
    scanner: Arc<Mutex<Scanner>>,
    /// Reads again now (project or period changed).
    kick: Option<async_channel::Sender<()>>,
}

impl UsageState {
    pub fn refresh(&self) {
        if let Some(k) = &self.kick {
            let _ = k.try_send(());
        }
    }
}

/// "周额度" for a 7-day window, "5 小时额度" for 300 minutes.
fn window_name(minutes: u64) -> String {
    match minutes {
        10_080 => "周额度".into(),
        m if m % 1440 == 0 => format!("{} 天额度", m / 1440),
        m if m % 60 == 0 => format!("{} 小时额度", m / 60),
        m => format!("{m} 分钟额度"),
    }
}

impl LilaApp {
    /// Starts the background reads (once, from `LilaApp::new`).
    pub fn start_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kick, kicked) = async_channel::unbounded::<()>();
        self.usage.kick = Some(kick);
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(job) = this.update(cx, |this, _| {
                    let p = this.project()?;
                    Some((p.id, p.cwd, this.usage.period, this.usage.scanner.clone()))
                }) else {
                    return;
                };
                if let Some((project, cwd, period, scanner)) = job {
                    let id = project.clone();
                    let usage = cx
                        .background_executor()
                        .spawn(async move {
                            let since = period.since(data::now_ms());
                            let mut s = scanner.lock().unwrap_or_else(|e| e.into_inner());
                            s.usage(&Sources::from_env(), &id, &cwd, since)
                        })
                        .await;
                    let shown = Shown { project, period, usage };
                    if this
                        .update(cx, |this, cx| {
                            // Dropped when the project or period moved on meanwhile (a fresh read is queued).
                            if this.selected.as_deref() == Some(&shown.project) && this.usage.period == shown.period {
                                this.usage.shown = Some(shown);
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                let asked = std::pin::pin!(kicked.recv());
                let timer = std::pin::pin!(cx.background_executor().timer(Duration::from_secs(30)));
                futures::future::select(asked, timer).await;
                while kicked.try_recv().is_ok() {}
            }
        })
        .detach();
    }

    pub fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {
        if self.usage.period != p {
            self.usage.period = p;
            self.usage.refresh();
        }
        cx.notify();
    }

    /// Small period switch for the section header: a segmented control, the
    /// chosen segment a raised thumb.
    pub fn usage_periods(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = self.usage.period;
        h_flex()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(8.))
            .bg(crate::style::well())
            .children(Period::ALL.into_iter().map(|p| {
                let on = p == now;
                div()
                    .id(SharedString::from(format!("usage-{}", p.label())))
                    .h(px(22.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_xs()
                    .font_medium()
                    .cursor_pointer()
                    .when(on, |d| d.bg(gpui::Hsla::from(rgba(0xffffff29))).text_color(crate::style::fg()).shadow_xs())
                    .when(!on, |d| d.text_color(crate::style::muted()).hover(|d| d.text_color(crate::style::fg())))
                    .child(p.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_period(p, cx)))
            }))
            .into_any_element()
    }

    pub fn usage_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let muted = |text: String| div().text_xs().text_color(t.muted_foreground).child(text);
        let current = self.selected.clone();
        let Some(shown) = self.usage.shown.as_ref().filter(|s| Some(&s.project) == current.as_ref() && s.period == self.usage.period) else {
            return div().px_4().pb_3().child(muted("正在读取…".into())).into_any_element();
        };
        // The team as lilad has it; unknown while it is down (then nobody is marked as gone).
        let team: Option<Vec<String>> = self.snap.status.as_ref().map(|_| self.members().iter().map(|m| s(m, "id")).collect());
        let u = &shown.usage;
        let now = data::now_ms();
        let max = u.members.iter().map(|m| m.tokens.total()).max().unwrap_or(1).max(1);
        let mut out = v_flex().px_4().pb(px(14.)).gap_2();
        if u.members.is_empty() {
            let when = match shown.period {
                Period::Today => "今天",
                Period::Week => "这 7 天",
                Period::All => "",
            };
            out = out.child(muted(format!("{when}还没有用量记录。成员开始工作后，这里按成员统计 Claude Code 和 Codex 用掉的 token。")));
        }
        for m in &u.members {
            let in_team = team.as_ref().is_none_or(|t| t.contains(&m.member));
            let models: Vec<&str> = m.models.keys().map(String::as_str).collect();
            let tk = m.tokens;
            let tip = format!(
                "{}\n输入 {} · 缓存写入 {} · 缓存读取 {} · 输出 {}\n{} 个会话 · 最近 {}",
                models.join("、"),
                short(tk.input),
                short(tk.cache_write),
                short(tk.cache_read),
                short(tk.output),
                m.sessions,
                data::local_when(m.last_ms, now)
            );
            let frac = tk.total() as f32 / max as f32;
            let bar = if m.runtime == "codex" { t.blue } else { t.primary };
            out = out.child(
                v_flex()
                    .id(SharedString::from(format!("usage-{}", m.member)))
                    .gap_1()
                    .tooltip(move |window, cx| gpui::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_size(px(13.)).flex_none().when(!in_team, |d| d.text_color(t.muted_foreground)).child(m.member.clone()))
                            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(t.muted_foreground).child(if in_team { models.join("、") } else { "已不在团队".into() }))
                            .child(div().text_size(px(13.)).font_medium().child(short(tk.total()))),
                    )
                    .child(div().h(px(3.)).w_full().rounded_full().bg(t.muted.opacity(0.6)).child(div().h_full().rounded_full().bg(bar.opacity(0.8)).w(relative(frac.max(0.02))))),
            );
        }
        if !u.members.is_empty() {
            out = out.child(h_flex().pt_1().gap_2().child(muted(format!("合计 {} tokens", short(u.total.total())))).child(div().flex_1()).child(muted(format!("其中缓存读取 {}", short(u.total.cache_read)))));
        }
        for l in u.codex_limits.iter().filter(|l| l.resets_at_ms > now) {
            out = out.child(limit_line(l, now, cx));
        }
        out.into_any_element()
    }
}

fn limit_line(l: &Limit, now: i64, cx: &App) -> AnyElement {
    let t = cx.theme();
    let used = l.used_percent;
    let color = if used >= 100. {
        t.red
    } else if used >= 80. {
        t.yellow
    } else {
        t.muted_foreground
    };
    let reset = data::local_when(l.resets_at_ms, now);
    let text = if used >= 100. {
        format!("Codex {}用完了，{reset} 重置。在那之前用 Codex 的成员跑不动。", window_name(l.window_minutes))
    } else {
        format!("Codex {}已用 {used:.0}%，{reset} 重置", window_name(l.window_minutes))
    };
    h_flex()
        .items_start()
        .gap_1p5()
        .px_2()
        .py_1p5()
        .rounded_md()
        .bg(t.muted.opacity(0.4))
        .child(div().pt(px(2.)).child(Icon::new(if used >= 80. { IconName::TriangleAlert } else { IconName::Info }).xsmall().text_color(color)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_xs().text_color(color).child(text))
                .child(div().text_xs().text_color(t.muted_foreground.opacity(0.7)).child(format!("{}的记录{}", data::local_when(l.seen_ms, now), l.plan.as_ref().map(|p| format!(" · {p} 套餐")).unwrap_or_default()))),
        )
        .into_any_element()
}
