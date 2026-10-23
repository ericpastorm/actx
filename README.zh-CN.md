<p align="center">
  <img src="assets/readme-hero.svg" alt="actX — 跨设备保留你的智能体对话上下文" width="100%">
</p>

# 随时随地，继续你的智能体对话

通过共享文件夹，在不同设备间同步 Claude Code、opencode 和 Codex 对话。
也可将对话转换到 Antigravity；目前 Antigravity 支持转换和本地关联副本，尚不支持原生共享同步。

[English](README.md) · [Español](README.es.md)

[桌面指南](docs/desktop.md) · [JSON API](docs/json-api.md) · [更新记录](CHANGELOG.md)

## 功能一览

| 功能 | 用途 |
| --- | --- |
| 跨设备同步 | 双向同步对话，识别仓库克隆并适配不同的项目路径。 |
| 切换智能体 | 在 Claude Code、Codex、opencode 与 Antigravity CLI 之间转换对话。 |
| 项目与常规对话 | 手动选择共享内容，也可启用带排除规则的项目自动添加。 |
| 原生桌面与终端 | 搜索、预览消息、查看副本状态，并在所选智能体中继续对话。 |
| 配置与自动化 | 每台设备独立配置智能体副本，按需启用 hooks 和定时同步。 |
| 冲突与恢复 | 比较历史、审阅解决方案，恢复备份、隔离区和中断的共享发布。 |
| 本地清理 | 审阅候选对话后，将选中的对话移入可恢复隔离区。 |
| JSON API | 为脚本和其他界面提供带版本的查询、预览和确认操作。 |

![actX 原生桌面，使用示例对话](assets/screenshots/desktop.png)

桌面采用 Pixel 5×7 标识、红色动态 X 和官方智能体图标。先显示本地目录，再后台检查更新。
[CLI 完整指南](docs/cli.md) · [桌面指南](docs/desktop.md) · [JSON API](docs/json-api.md)

## 快速开始

```sh
actx init /path/to/shared --lang zh-CN
cd /path/to/project
actx mk myproject
```

`mk` 将当前工作文件夹及其仓库关联到项目，然后让你选择要保存的对话。
默认只显示 actx 正在维护的对话；用 `--all` 查看所有本地对话。

```sh
actx                         # 浏览已维护的对话
actx add                     # 向当前项目添加对话；项目外保存到常规
actx add --general           # 明确保存为无项目对话
actx sync --dry-run          # 预览，不修改数据
actx sync                    # 同步当前项目；在项目外选择范围
actx sync --general          # 同步无项目对话，并选择此设备上的目标文件夹
actx resume                  # 同步并继续对话
actx cv                      # 转换当前文件夹/项目中的对话，无需配置共享存储
actx cv --all                # 所有文件夹，包括没有工作目录的对话
actx delete                  # 删除所选共享副本，保留本地原始对话和项目
actx rm myproject            # 删除整个项目及共享副本；需要确认
```

每台设备都需要配置同一共享文件夹。actx 会尝试识别不同路径或名称的仓库克隆；
如果无法确定目标位置，会询问并记住选择。代码本身不会随对话同步，请单独提交和推送代码。

可通过系统语言、`ACTX_LANG=zh-CN` 或 `actx init --lang zh-CN` 使用简体中文。
支持 `zh_CN.UTF-8`、`zh-Hans` 和 `zh-SG` 等系统语言形式；不将繁体中文标记为已翻译。
终端和 fzf 需要可显示中文的字体。列表按可见列宽对齐和截断，保留 JSON 标识与状态码不变。

## 脚本和其他界面

可选桌面界面：通过以下命令安装并打开：

```sh
cargo install --git https://github.com/ericpastorm/actx --features gui --locked
actx gui
```

也可以用 `cargo build --release --features gui --locked` 从源码编译。
安装会提供 `actx` 和可双击的 `actx-gui` 启动器；手动复制时请将两者放在同一目录。
支持项目、常规对话、搜索、消息预览、添加、删除、同步、转换和继续对话。
写入前均预览并确认。窄窗口使用对话/详情标签页，Ctrl+1 / Ctrl+2 切换。
首次打开可选择共享文件夹，也可直接使用转换功能。
启动时先显示本地保存的目录，再在后台验证；仅在选择对话后读取消息。
可比较冲突、将所选本地对话移入可恢复的隔离区，并恢复中断的共享发布。
清理支持进度与取消，已完成的移动仍可恢复。
桌面原生运行已在 Linux 上检查；Windows/macOS 的窗口、对话框和终端启动仍待实际验证。
CI 在三个系统上测试 CLI，并编译 GUI、运行 Clippy 及界面状态与绘制测试。
Linux 还通过 Xvfb 启动真实窗口，检查目录刷新及共享文件夹离线时的数据保留。
参见[桌面预览说明和当前限制](docs/desktop.md)。

```sh
actx ls --json
actx sync --dry-run --json
echo '{"action":"list"}' | actx api
```

JSON 使用独立于界面语言的标识、状态和原因码。写入操作需要先预览，再发送对应版本的确认。
完整字段和退出码见 [JSON 接口](docs/json-api.md)。

## 注意事项

共享文件夹中的对话可能包含命令输出、密钥或令牌，请保持私密。
转换会将工具调用变为文本，并可能省略推理、图像和压缩前的消息。
冲突不会被静默覆盖；请查看待处理状态并选择处理方式。
完整安装和平台说明见 [英文文档](README.md)。中文译文仍欢迎母语使用者审校。

## 自动添加

默认手动选择。按项目选择启用：`actx auto-add MyProject --on --exclude 'scratch/**'`。
保存规则不会立即添加会话；下次同步项目或全部项目时生效，每台设备使用自己的项目路径。
加上 `--json` 仅预览而不保存；`--off` 停用规则并保留已保存的会话。
模糊匹配、正在使用或未完成的会话会被推迟。桌面界面提供相同的规则编辑器。
参见[规则与排除](docs/json-api.md#automatic-additions-opt-in-per-project)。

## 冲突与恢复

`actx compare claude:SESSION-ID` 比较本地与共享历史，`actx resolve claude:SESSION-ID`
可选择处理方式并在确认后执行。`--target codex` 用于比较已关联的目标副本。
`actx recoveries` 列出备份、隔离区和可识别的中断发布；恢复前先审阅版本。
恢复共享发布时会保留当前共享版本作为另一个恢复条目，本地原始对话不受影响。
不完整或仍在写入的残留会保留；`actx doctor` 可报告待处理发布。
