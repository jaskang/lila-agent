//! Flows: the steps a piece of work goes through (implement → check → verify),
//! run by lilad rather than remembered by the members. A member does its step
//! and reports; lilad starts the next one, runs the check command itself, sends
//! failures back to whoever did the work, and gives up after `max_rounds`.
//!
//! ```toml
//! [flows.code]
//! steps = [
//!   { do = "implement", by = "engineer" },
//!   { do = "check" },                                  # lilad runs `[settings] check`
//!   { do = "verify", by = "tester", on_fail = "implement" },
//! ]
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The step lilad runs itself: the project's check command.
pub const CHECK: &str = "check";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// What the step does: `check`, or a name for a member's step (`implement`, `verify`, ...).
    #[serde(rename = "do")]
    pub action: String,
    /// The member (or pool, `engineer` with `count = 2`) that does it. None for `check`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// This step gives a verdict; a fail goes back to the step named here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_fail: Option<String>,
}

impl Step {
    pub fn is_check(&self) -> bool {
        self.action == CHECK
    }

    /// Ends with `--verdict pass|fail`.
    pub fn wants_verdict(&self) -> bool {
        self.on_fail.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    pub steps: Vec<Step>,
}

fn member_step(action: &str, by: &str, on_fail: Option<&str>) -> Step {
    Step { action: action.into(), by: Some(by.into()), on_fail: on_fail.map(Into::into) }
}

fn check_step() -> Step {
    Step { action: CHECK.into(), by: None, on_fail: None }
}

/// The flows every project has unless it defines its own of the same name.
/// Steps for members the team lacks are left out (no tester: no verify).
pub fn builtin(has: impl Fn(&str) -> bool) -> BTreeMap<String, Flow> {
    let mut out = BTreeMap::new();
    if !has("engineer") {
        return out;
    }
    let mut code = vec![member_step("implement", "engineer", None), check_step()];
    if has("tester") {
        code.push(member_step("verify", "tester", Some("implement")));
    }
    out.insert("code".into(), Flow { steps: code });
    out.insert("quick".into(), Flow { steps: vec![member_step("implement", "engineer", None), check_step()] });
    out
}

/// Checks a flow against the team. `is_target(id)`: a member or pool id.
pub fn validate(name: &str, flow: &Flow, is_target: impl Fn(&str) -> bool) -> Result<(), String> {
    let first = flow.steps.first().ok_or_else(|| format!("flows.{name}: no steps"))?;
    if first.is_check() {
        return Err(format!("flows.{name}: the first step must be a member's, not `check`"));
    }
    for (i, s) in flow.steps.iter().enumerate() {
        match (&s.by, s.is_check()) {
            (Some(_), true) => return Err(format!("flows.{name}: `check` is run by lilad; drop its `by`")),
            (None, false) => return Err(format!("flows.{name}: step {:?} needs `by`", s.action)),
            (Some(by), false) if !is_target(by) => return Err(format!("flows.{name}: step {:?}: no member {by:?}", s.action)),
            _ => {}
        }
        if s.is_check() && s.on_fail.is_some() {
            return Err(format!("flows.{name}: a failed `check` always goes back to the step before it"));
        }
        if let Some(back) = &s.on_fail
            && !flow.steps[..i].iter().any(|e| &e.action == back && !e.is_check())
        {
            return Err(format!("flows.{name}: step {:?}: on_fail = {back:?} is not an earlier member step", s.action));
        }
    }
    Ok(())
}

/// What lilad does after a step of a run ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Give `steps[step]` to its member.
    Member { step: usize },
    /// Run the check command (`steps[step]` is `check`).
    Check { step: usize },
    /// A fail: give `steps[step]` back to the member that did it. `round` counts send-backs.
    Back { step: usize, round: u32 },
    /// Every step passed.
    Finished,
    /// Failed more than `max_rounds` times: the PM decides.
    GiveUp,
}

/// After `steps[at]` ended (`passed`: its verdict or the check's exit). A check
/// step is skipped when there is no check command.
pub fn after(flow: &Flow, at: usize, passed: bool, round: u32, max_rounds: u32, has_check: bool) -> Next {
    if !passed {
        let back = match &flow.steps[at].on_fail {
            Some(name) => flow.steps[..at].iter().rposition(|s| &s.action == name),
            None => flow.steps[..at].iter().rposition(|s| !s.is_check()),
        };
        return match back {
            None => Next::GiveUp,
            Some(_) if round + 1 > max_rounds => Next::GiveUp,
            Some(step) => Next::Back { step, round: round + 1 },
        };
    }
    match flow.steps.iter().enumerate().skip(at + 1).find(|(_, s)| has_check || !s.is_check()) {
        None => Next::Finished,
        Some((i, s)) if s.is_check() => Next::Check { step: i },
        Some((i, _)) => Next::Member { step: i },
    }
}

/// The steps after `at` a member should know about, for its delivery: who gets
/// the work next and whether lilad runs the check (`check`: the command).
pub fn later(flow: &Flow, at: usize, check: Option<&str>) -> Vec<String> {
    flow.steps
        .iter()
        .skip(at + 1)
        .filter_map(|s| match (&s.by, s.is_check()) {
            (_, true) => check.map(|c| format!("runs `{c}`")),
            (Some(by), false) => Some(format!("sends it to `{by}` to {}", s.action)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> Flow {
        builtin(|_| true).remove("code").unwrap()
    }

    #[test]
    fn builtin_flows_fit_the_team() {
        assert_eq!(code().steps.iter().map(|s| s.action.as_str()).collect::<Vec<_>>(), ["implement", "check", "verify"]);
        let no_tester = builtin(|m| m != "tester");
        assert_eq!(no_tester["code"].steps.len(), 2, "no tester, no verify");
        assert!(builtin(|m| m != "engineer").is_empty());
        for (name, f) in builtin(|_| true) {
            validate(&name, &f, |_| true).unwrap();
        }
    }

    #[test]
    fn validation_catches_broken_flows() {
        let f = |steps: Vec<Step>| Flow { steps };
        assert!(validate("x", &f(vec![]), |_| true).is_err());
        assert!(validate("x", &f(vec![check_step()]), |_| true).unwrap_err().contains("first step"));
        assert!(validate("x", &f(vec![member_step("a", "ghost", None)]), |m| m != "ghost").unwrap_err().contains("no member"));
        let bad_back = f(vec![member_step("a", "e", None), member_step("v", "t", Some("design"))]);
        assert!(validate("x", &bad_back, |_| true).unwrap_err().contains("not an earlier member step"));
        let no_by = f(vec![Step { action: "a".into(), by: None, on_fail: None }]);
        assert!(validate("x", &no_by, |_| true).unwrap_err().contains("needs `by`"));
    }

    #[test]
    fn passing_steps_move_on_and_skip_a_missing_check() {
        let f = code();
        assert_eq!(after(&f, 0, true, 0, 3, true), Next::Check { step: 1 });
        assert_eq!(after(&f, 0, true, 0, 3, false), Next::Member { step: 2 });
        assert_eq!(after(&f, 1, true, 0, 3, true), Next::Member { step: 2 });
        assert_eq!(after(&f, 2, true, 0, 3, true), Next::Finished);
    }

    #[test]
    fn failures_go_back_until_the_rounds_run_out() {
        let f = code();
        assert_eq!(after(&f, 1, false, 0, 3, true), Next::Back { step: 0, round: 1 }, "a red check back to the implementer");
        assert_eq!(after(&f, 2, false, 1, 3, true), Next::Back { step: 0, round: 2 }, "on_fail names the step");
        assert_eq!(after(&f, 2, false, 3, 3, true), Next::GiveUp);
        let lone = Flow { steps: vec![member_step("review", "t", Some("review"))] };
        assert_eq!(after(&lone, 0, false, 0, 3, true), Next::GiveUp, "nothing earlier to go back to");
    }

    #[test]
    fn a_member_hears_what_lila_does_next() {
        let f = code();
        assert_eq!(later(&f, 0, Some("cargo test")), ["runs `cargo test`", "sends it to `tester` to verify"]);
        assert_eq!(later(&f, 0, None), ["sends it to `tester` to verify"]);
        assert!(later(&f, 2, Some("x")).is_empty());
    }
}
