# Docker 管理

在侧栏打开 Docker 页面，选择环境后查看容器，或启动、停止、重启容器、查看最近 200 行日志。停止和重启会中断容器中的服务。列表包含正在运行和已停止的容器；操作完成后会刷新，外部变更需要手动刷新。

页面使用紧凑卡片，镜像和端口默认单行省略，点击“详情”可查看并复制完整名称、ID、镜像和端口。搜索匹配名称或镜像（忽略大小写及首尾空白），支持全部、运行中、已停止筛选；筛选不会触发 Docker 命令。“已停止”对应 `exited`，其他状态仍可在“全部容器”中查看。

日志在独立弹窗中展示，等宽字体、默认不换行，支持横向滚动、刷新、复制和 Esc 关闭；读取失败会保留原日志并显示错误。文本框配色随应用深浅色主题同步。容器操作在对应卡片上显示进度，结束后提示结果；操作期间暂时禁用其他 Docker 请求。复制操作复用项目现有的 Windows 剪贴板工具。

## 环境要求

- **Docker Desktop**：Windows 能通过 PATH 找到 `docker.exe`，Desktop 已启动并处于 Linux 容器模式。页面显式使用 `desktop-linux` context，不切换全局 context。
- **WSL Docker Engine**：发行版为 WSL 2，已安装 Docker CLI，本地 Engine 已启动。默认使用发行版默认用户连接 `/var/run/docker.sock`；权限不足时，可在页面勾选“以 root 用户访问 Docker”，工具将通过 `wsl --user root` 执行查询、容器启停和日志读取，不需要手动修改用户组或 socket 权限。
- root 访问默认关闭，仅对当前选中的 WSL 环境有效，切换环境或重启应用后恢复关闭。开启时会明确显示 root 权限提示；切换身份会清空旧数据并重新检测，操作进行中不能切换身份。
- 发行版列表来自仪表板现有实例列表，刷新 Docker 页面会同步可选发行版；内部 `docker-desktop*` 发行版不作为普通 Engine 目标。
- 选择 WSL 环境会执行发行版内的命令，可能启动原本停止的发行版。Docker Desktop 的 WSL 集成也可能让多个入口连接同一个引擎，这些入口中的容器并非独立副本。

## 实现范围

复用现有侧栏、Slint 控件、Tokio 后台任务和国际化机制，新增独立 Docker 命令模块、UI handler 和页面。保留原有页面编号，Docker 使用编号 9；不新增持久化配置或依赖。

命令通过参数数组执行，不经过 shell。Desktop 固定指定 context；WSL 固定本地 socket，并清除 Docker 连接环境变量。容器列表使用 `docker ps --all --no-trunc --format '{{json .}}'` 逐行解析 JSON，仅接受完整容器 ID 和允许的操作。

切换目标会清空旧环境的列表及日志；执行期间禁用目标切换和重复操作，防止结果与目标不一致。后台命令限时 45 秒，每个输出流最多读取 1 MiB，超限时明确报错而非展示不完整的列表。日志同时保留 stdout 和 stderr，但不保证两者合并后的时间顺序。超时会结束宿主 CLI 进程，WSL 内或引擎侧操作可能继续，需刷新确认状态后再重试。

普通刷新保留搜索、筛选、展开项及列表滚动位置，数据减少时滚动位置会受剩余内容高度限制。刷新失败时保留上次结果并显示错误，此时不能操作旧数据，需要先刷新成功。切换环境或 root 身份时清空列表、搜索、展开项、滚动位置和日志。

第一版不包含 rootless 或自定义 socket、远程 context、Windows 容器、Compose、镜像管理、自动安装、删除容器、数据卷清理和实时日志跟随。

## 验证

自动检查：`cargo test --locked docker::tests` 和 `cargo check --locked`。测试覆盖容器 JSON、无效输出、ID/操作校验、目标参数隔离、搜索与状态筛选及输出上限，不会连接真实 Docker 或改变容器状态。

人工验收（在测试容器上执行）：

1. 访问其他页面时不应触发 Docker 检测；首次进入 Docker 页面检测 Desktop。
2. Desktop Linux 引擎启动时展示容器；未安装 CLI、引擎停止或权限不足时显示错误。
3. 选择安装有 Engine 的 WSL 2 发行版，核对列表与该发行版终端内的 `docker ps -a` 一致。
4. 启动、停止、重启测试容器后核对状态；日志应包含 stdout 与 stderr，且支持选择复制。
5. 切换环境时旧列表和日志消失，执行中不能切换或重复操作；空环境应显示空状态。
6. CLI 当前 context 指向其他引擎时，页面仍应操作指定的 Desktop 或 WSL 本地引擎。
7. 默认用户无 socket 权限时，勾选 root 访问后应能连接已运行的本地引擎；取消勾选后重新以默认用户检测。切换环境时开关恢复关闭，Docker Desktop 不显示此选项。
8. 检查搜索与状态筛选组合、无匹配提示、展开后完整信息及复制结果；普通刷新保持展开项与滚动位置，切换环境则重置。
9. 打开日志弹窗并刷新、复制、关闭；长日志保持不换行并可水平滚动，关闭后列表恢复完整高度，读取错误不清空旧日志。

命令格式参考：[容器列表](https://docs.docker.com/reference/cli/docker/container/ls/)、[Docker contexts](https://docs.docker.com/engine/manage-resources/contexts/)、[容器日志](https://docs.docker.com/reference/cli/docker/container/logs/)。
