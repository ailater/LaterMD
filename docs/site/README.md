# LaterMd 下载页

无框架、无构建步骤的静态下载页。页面资源均在本地，不依赖外部字体、图标 CDN 或第三方脚本。

## 本地预览

```sh
python3 -m http.server 4173 --bind 0.0.0.0
```

服务监听所有 IPv4 网卡。本机打开 `http://localhost:4173/`，局域网设备可通过 `http://<本机局域网 IP>:4173/` 访问，也可以直接打开 `index.html`。

## 文件

- `index.html`：内容、下载入口、可切换的功能示意与安装说明。
- `styles.css`：蓝白配色、柔和背景与响应式布局。
- `script.js`：系统识别、示意图标签切换、移动菜单、命令复制。
- `assets/latermd-icon.png`：来自 LaterMd 仓库的品牌图标。

## 维护

当前下载链接固定到 `v0.0.10`（2026-10-24 发布），不会自动跟随新版本。更新版本时，应一起更新 `index.html` 中的发布信息、系统要求、安装包文件名和 Release 链接，并核对 Release 中实际存在的资产。

产品展示明确标记为「功能示意 · 非产品截图」，不代表真实 UI 的逐像素还原。后续可以替换为团队确认的真实截图。

平台识别仅根据浏览器提供的系统信息推荐，不能保证系统版本或硬件兼容性。手机和平板不会被推荐安装桌面版本。

页面可直接部署到任何静态服务器，保留上述文件的相对位置即可。复制命令优先使用安全上下文中的 Clipboard API，不可用时尝试本地复制回退，失败会提示手动复制。

## 部署

线上地址为 `https://md.ailater.com`，由 `.github/workflows/pages.yml` 在 push 到 `main`（本目录有变更时）自动发布：只上传 `docs/site/` 子目录，`docs/` 下其余内部规划文档不进产品域名。仓库 Pages 源设置为 GitHub Actions，自定义域名 `md.ailater.com` 在仓库 Pages 设置中绑定；DNS 侧需在域名服务商处将 `md.ailater.com` CNAME 到 `ailater.github.io`。
