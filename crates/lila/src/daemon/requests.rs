//! What lilad answers: every request from the CLI, the window and the engines (`Daemon::handle`).
//! The methods and their parameters are written down in docs/architecture-v4.md §4.

use super::*;

impl<T: Terminals> Daemon<T> {
    /// A request answered now: work that has to be run off the loop (`begin`) is run here, one thing at a time (tests; the server runs it on threads).
    pub fn handle(&mut self, method: &str, params: &Value, now: i64) -> Result<Value> {
        match self.begin(method, params, now)? {
            Begin::Done(v) => Ok(v),
            Begin::Job(job) => {
                let result = job.run();
                self.finish(job, result, now)
            }
        }
    }

    /// Every request that needs no job.
    pub(super) fn dispatch(&mut self, method: &str, params: &Value, now: i64) -> Result<Value> {
        match method {
            "ping" => Ok(json!({"pong": true, "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"), "protocol": crate::PROTOCOL})),
            "status" => self.status(now),
            "project.list" => Ok(Value::Array(self.projects.keys().map(|id| self.project_json(id, now)).collect::<Result<Vec<_>>>()?)),
            "project.show" => {
                let pid = self.resolve_project(params)?;
                self.project_json(&pid, now)
            }
            "project.add" => self.project_add(params, now),
            "project.remove" => self.project_remove(params, now),
            "project.reload" => self.project_reload(params, now),
            "project.set" => self.project_set(params, now),
            "thread.list" => self.thread_list(params, now),
            "thread.show" => {
                let t = self.thread_of(params)?;
                Ok(self.thread_json(&t, now))
            }
            "thread.read" => self.thread_read(params, now),
            "thread.send" => self.thread_send(params, now),
            "thread.interrupt" => self.thread_interrupt(params),
            "thread.stop" => self.thread_stop(params, now),
            "thread.resume" => self.thread_resume(params, now),
            "thread.archive" => self.thread_archive(params, true, now),
            "thread.unarchive" => self.thread_archive(params, false, now),
            "thread.rename" => self.thread_rename(params, now),
            "thread.permission" => self.thread_permission(params, now),
            "thread.answer" => self.thread_answer(params, now),
            "thread.model" => self.thread_model(params),
            "thread.run" => self.thread_run(params),
            "thread.commands" => self.thread_commands(params),
            "thread.files" => self.thread_files(params),
            "thread.file" => self.thread_file(params),
            "thread.log" => self.thread_log(params),
            "thread.checkpoints" => self.thread_checkpoints(params, now),
            "hook" => self.hook(params, now),
            "hook.answer" => self.hook_answer(params),
            other => Err(anyhow!("unknown request {other}")),
        }
    }

    /// `status`: lilad itself, and how many threads are in each state.
    fn status(&self, now: i64) -> Result<Value> {
        let mut states: BTreeMap<&str, usize> = BTreeMap::new();
        for p in self.projects.keys() {
            for t in self.store.threads(p, false)? {
                *states.entry(self.state_of(&t, now).0.as_str()).or_default() += 1;
            }
        }
        Ok(json!({"pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"), "protocol": crate::PROTOCOL, "projects": self.projects.len(), "threads": states, "cards": self.asking.len(), "magpie": self.magpie.is_some()}))
    }
}
