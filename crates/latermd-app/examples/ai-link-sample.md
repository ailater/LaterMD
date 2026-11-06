# ai:// 链接冒烟样例

用 LaterMD 打开本文件(文件 → 打开),在写作模式里点链接体验
(分栏档下右侧的只读对照列同样可点):

## 已实现:write 触发 Mock 流式续写

- [点我续写一段 Markdown 介绍](ai://write?prompt=%E7%BB%AD%E5%86%99%E4%B8%80%E6%AE%B5%20Markdown%20%E4%BB%8B%E7%BB%8D)
- [提示词含标点与空格](ai://write?prompt=%E6%80%BB%E7%BB%93%2C%E6%9C%AC%E6%96%87%E8%A6%81%E7%82%B9%21%28%E5%90%AB%E6%A0%87%E7%82%B9%2B%E7%A9%BA%E6%A0%BC%29)

点击后 AI 文本流式追加到本文档末尾;流式进行中再点任何 AI 链接会被防重入忽略。

## 已识别但未实现:点击只提示

- [ai://summarize —— 提示「未实现的 AI 动作」](ai://summarize?prompt=%E6%91%98%E8%A6%81%E6%9C%AC%E6%96%87)
- [ai://write 缺 prompt —— 提示解析失败](ai://write)

## 不拦截:普通链接照常走浏览器

- [LaterMD 仓库](https://github.com/ailater/LaterMd)

样式上 `ai://` 链接显示为紫罗兰色(明暗主题各一档),普通链接保持默认超链接色。
协议语义定稿见 docs/decisions-pending.md #11。
