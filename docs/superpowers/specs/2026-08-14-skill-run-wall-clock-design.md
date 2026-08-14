# Skill Run Wall-Clock Time Design

## Goal

让 Skill Run 能在同一批日志中按用户看到的故障时间定位上下文，不把日志时间解释为跨时区的绝对时间。

## Scope

时间范围仍属于 Skill Run。创建 Run 时保存范围，Runner 读取不可变的 Trusted Run Scope，`search_logs` 由服务端自动套用范围，保留最多 15 分钟的 bounded context expansion、`event_time_indexed` backfill 状态、coverage 提示和无范围 Run 的旧行为。

## Time model

API 接受没有时区的本地 wall-clock 文本：日期、小时、分钟、秒，可选小数秒；日期和时间之间允许空格或 `T`。前端 `datetime-local` 的分钟值补为零秒后原样保留其墙上时间语义。服务端只做格式规范化，不做 `toISOString()`、UTC 转换或 RFC3339 时区校验。

内部时间使用 `NaiveDateTime` 做日历运算，并编码成单调可比较的 wall-clock comparison key。整数值只服务于 `start <= event_time <= end` 的比较和 SQLite 索引，不代表 Unix epoch、UTC 或任何跨设备绝对时间。现有数据库 `*_ms` 列名保留，以避免不必要的 schema rename；代码注释、错误信息、文档和工具输出不得再暗示其为 epoch milliseconds。

## Log parsing

索引器支持行首的 `YYYY-MM-DD HH:mm:ss[.fraction]`、`YYYY-MM-DDTHH:mm:ss[.fraction]`、包在一层方括号中的时间，以及 `[E][时间][...]` 这类日志前缀。没有日期、只有 `HH:mm:ss` 的日志不推断日期，仍进入 unindexed coverage。一个 segment 保存可解析行的最小/最大 wall-clock key。

## Validation and errors

`start`、`end` 必须存在且可解析，`start < end`，窗口不超过 24 小时。无效格式统一返回现有 `INVALID_TIME_SCOPE` API 错误。未设置范围的 Run 不改变原有搜索行为。

## Testing

测试覆盖无时区 API 输入、`T`/空格分隔、毫秒、分钟精度、边界和 expansion；覆盖三类日志前缀、无日期时间的保守拒绝、backfill/indexed 状态、scoped filtering/coverage；前端验证不产生 UTC 字符串且保留本地墙上时间。
