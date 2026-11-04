---
status: accepted
---

# SearXNG compose 归入本项目，不新增容器

现有 `<旧 SearXNG Compose 目录>` 是一个独立 Compose 项目（`name: searxng`，容器 `searxng-core` / `searxng-valkey`，端口 12310）。本项目把它的 `docker-compose.yml`、`.env`、`core-config/` 迁入 `deploy/searxng/`，保留 `name: searxng` 与 `container_name`，因此仍是同一个容器和同一组命名卷，不新增容器项目。

## Considered Options

- 保持外部项目，仅通过 HTTP 复用：零迁移风险，但仓库不自包含，部署依赖另一个目录。
- 新建一个 SearXNG compose 项目：会产生第二个容器/端口/卷，违背“不额外启用容器项目”的约束。
- 把 SearXNG 嵌入 Rust 二进制：不可行，见 ADR 0003。
- 迁入本项目（采纳）：仓库自包含，容器数量不变；代价是一次性迁移和端口/容器名约束。

## Consequences

- `deploy/searxng/docker-compose.yml` 必须保留 `name: searxng` 和 `container_name: searxng-core`；否则会新建项目并占用 12310。
- 迁移必须先 `down` 再 `up`；`down` 不带 `-v` 以保留命名卷。
- `core-config` 必须随 compose 文件移动（bind mount 相对路径）。
- `searxng-valkey` 当前未被 core 使用，可从 compose 删除以少一个空闲容器。
