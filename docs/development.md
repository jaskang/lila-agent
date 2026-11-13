# 开发说明

架构见 `architecture.md`，计划见 `plan.md`，界面规范见根目录的 `DESIGN.md`。

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

## 不点鼠标验证界面

窗口本身就是设计稿，改完界面要截图自查。用 `cargo build -p lila-app --features shots` 编译出的窗口可以截自己，也能收模拟的键盘和鼠标事件。

| 环境变量 | 作用 |
|---|---|
| `LILA_APP_SHOT=/path.png` | 每 2 秒把窗口画面存成 PNG（2882 × 1802，2 倍像素） |
| `LILA_APP_FAKE=/path.json` | 不连后台，用造好的数据画界面。JSON 是 `{projects, threads, conversations}`，前两个和 `project.list`、`thread.list` 的返回一样；`conversations` 是会话 id 到 `thread.read` 的返回。能造的其他键见 `crates/lila-app/src/data.rs` 的 `fake_act` |
| `LILA_APP_THREAD=<id>` | 显示这个会话 |
| `LILA_APP_SIZE=1000x680` | 窗口大小，用来看窄窗口 |
| `LILA_APP_CLICK="x,y;right:x,y;move:x,y;keys:cmd-n h i enter;wait:ms"` | 启动 3 秒后依次点击、右击、移动、按键、等待。坐标是窗口里的点，是截图像素的一半 |
| `LILA_APP_KEYS="cmd-, cmd-shift-,"` | 启动 1.5 秒后按这些键 |
| `LILA_APP_INACTIVE=1` 加 `LILA_APP_NOTIFY_LOG=<文件>` | 窗口当作不在前面，通知写进文件而不真的发 |
| `LILA_APP_CONV=…` | 对话和输入框的状态，见 `conversation::dev_conv`、`composer::dev_composer` |

注意：

- 显示器睡着时窗口不重绘，截到的一直是第一帧。先看 `pmset -g assertions`。
- 刚编译出的程序第一次启动慢，至少等 8 秒再截。
- macOS 没有 `timeout`：`prog & P=$!; sleep 6; kill $P`。裁剪截图用 `sips -c H W --cropOffset Y X`。

## 用真 omp 跑

要省额度，并且不碰平时用的数据：

- 放在单独的 `LILA_HOME`（一个临时目录）里跑。
- 能用假 omp 测的不用真的：`crates/lila/tests/omp.rs` 里有一个按脚本回帧的假 omp（通过 `OmpAgents` 启动），`crates/lila/src/daemon/tests.rs` 里服务核心的测试用 `Fake` 代替 omp。
- 跑完把自己起的进程都结束掉：`lila thread stop <id>`，再 `lila daemon stop`。
