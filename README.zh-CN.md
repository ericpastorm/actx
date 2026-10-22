# actx

通过共享文件夹，在不同设备间同步 Claude Code、opencode 和 Codex 对话。
也可将对话转换到 Antigravity；目前 Antigravity 支持转换和本地关联副本，尚不支持原生共享同步。

[English](README.md) · [Español](README.es.md)

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
actx cv --all                # 转换对话，也可不配置共享存储
actx delete                  # 删除所选共享副本，保留本地原始对话和项目
actx rm myproject            # 删除整个项目及共享副本；需要确认
```

每台设备都需要配置同一共享文件夹。actx 会尝试识别不同路径或名称的仓库克隆；
如果无法确定目标位置，会询问并记住选择。代码本身不会随对话同步，请单独提交和推送代码。

可通过系统语言、`ACTX_LANG=zh-CN` 或 `actx init --lang zh-CN` 使用简体中文。
支持 `zh_CN.UTF-8`、`zh-Hans` 和 `zh-SG` 等系统语言形式；不将繁体中文标记为已翻译。
终端和 fzf 需要可显示中文的字体。列表按可见列宽对齐和截断，保留 JSON 标识与状态码不变。

## 脚本和其他界面

可选桌面界面：通过 `cargo build --release --features gui --locked` 编译，运行 `actx gui` 打开。
支持项目、常规对话、搜索、消息预览、添加、删除、同步、转换和继续对话。
写入前均预览并确认。窄窗口使用对话/详情标签页，Ctrl+1 / Ctrl+2 切换。
首次打开可选择共享文件夹，也可直接使用转换功能。
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
