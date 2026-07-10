# Rym

Rym 是一门为 LoongArch 指令集设计的系统编程语言，也是 Rymos 操作系统的实现语言。

## 核心设计铁律

- **零 FFI，零外部 ABI** — 不支持 C ABI，不依赖任何外部编译产物
- **双区架构** — 源文件物理分为定义区（上）和算法区（下）
- **绝对平坦化** — 禁止多行块嵌套，复杂逻辑提取为命名函数
- **管道符连接** — `|>` 是算法区唯一的拓扑连接符
- **显式 Allocator** — 每个分配函数必须显式接收 Allocator 参数
- **显式错误处理** — `or_return / or_panic / or_else / or_zero / or_nil`
- **双环隔离** — `safe` 环自动安全，`base` 环极致底层

## 目标平台

**龙芯 LoongArch64**（LA64）

## 仓库结构

```
compiler/   rymc 编译器（引导阶段用 C，自举后用 Rym 本身）
stdlib/     标准库
spec/       语言规范文档
```

## 关联项目

- [rymos](https://github.com/anxiong2025/rymos) — 用 Rym 实现的异步框内核操作系统
