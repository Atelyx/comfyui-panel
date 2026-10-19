# ComfyUI

Atelyx 原生插件（Rust 编译为平台可执行文件，经 stdio 上的 JSON-RPC 与宿主通信）：在 Atelyx 里选工作流、调参数、排队生成、看实时进度，并把结果存进当前仓库。需要节点级编排时，一键在系统浏览器打开 ComfyUI 原生界面。

> 要求 Atelyx 0.6.0 及以上。

## 界面

- **ComfyUI 生成**面板：工作流、参数、队列、进度、结果画廊。
- **ComfyUI 设置**页：地址端口、进程托管、保存目录。

两者在「添加面板」里分别添加。

## 用法

工作流直接读自 ComfyUI 用户目录（`user/default/workflows/`），无需导入。

1. 在 ComfyUI 里编排并保存，几秒内自动出现在面板的工作流列表里。
2. 选工作流、调参数（表单按节点定义自动生成），点「生成」，结果进入下方画廊。
3. 画廊三个动作：**存到仓库**写入当前仓库的保存目录、**另存为**自选位置落盘、**大图预览**在独立窗口按原始尺寸查看（需本地副本就绪）。
4. 「保存参数」把面板里的参数写回工作流文件；UI 格式只改 widget 值，结构与连线不动。
5. 工作流被外部改动时给出提示，重新选择即载入最新内容。

LoadImage 类节点的 `image` 输入带**上传**、**粘贴**、**清除**：上传选本地图片，粘贴取剪贴板里的图片，两者都经 ComfyUI 的 `/upload/image`。

含子图的工作流不能在面板内转换，请用「在浏览器打开」到原生界面编排。

## 连接

用 Rust 的 HTTP/WebSocket 客户端直连本机 ComfyUI，默认 `127.0.0.1:8188`，地址与端口在设置页改。所有请求只发往该地址，不做任何联网上传；工作流列表与文件读写走 ComfyUI 自身的 `/userdata` 接口。

## 进程

两种托管方式：**托管启动**由插件启停，命令为 `python main.py --listen <地址> --port <端口>` 再追加自定义参数，停止时结束整棵进程树；**外部管理**自行启动，插件只检测连接。安装目录已填、Python 路径留空时，自动取该目录 venv 里的解释器（Windows `venv/Scripts/python.exe`、类 Unix `venv/bin/python`）。插件停用、卸载或应用退出时，Atelyx 会收尾它启动的进程。

## 权限

| 声明 | 用途 |
| --- | --- |
| `process` | 启停 ComfyUI 进程树；调系统命令打开浏览器 |
| `state` | 保存设置与生成历史 |
| `notification` | 状态提示 |
| `dialog` | 选择安装目录、参考图与结果另存位置 |
| `fs` | 读写图片文件：参考图、结果副本、保存与另存 |
| `window` | 大图预览窗口 |

## 构建

```sh
cargo build --release   # 产物 target/release/comfyui-panel(.exe)
cargo test              # 纯逻辑单元测试
```

依赖同级 `../../Atelyx` 的 `atelyx-plugin-sdk` / `atelyx-plugin-protocol`。把产物与 `plugin.json` 一并放进插件目录即可装载。
