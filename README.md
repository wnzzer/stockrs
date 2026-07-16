# stockrs

轻量级 A 股量化 CLI 工具，纯 Rust 编写，编译为单二进制，下载即用。

数据获取与维护由工具本身完成，用户只需编写 [Rhai](https://rhai.rs) 策略脚本，CLI 负责确定性的回测执行与结果输出。

> **核心理念：** 逻辑在代码里，不在 LLM 里。所有计算、回测、信号判断都是确定性的，同样的输入永远产生同样的输出。

## 特性

- 📊 多数据源容灾：东方财富为主，腾讯、新浪自动故障切换，无需 API Key
- 💾 SQLite 单文件存储，零部署
- 📈 内置技术指标：MA / EMA / RSI / MACD / KDJ / BOLL
- 💹 基本面数据：历史日度 PE / PB / PS / 总市值，支持技术面 + 基本面双重验证回测
- 🇭🇰 港股支持：行情 / 日K / 基本面(PE/PB) / 回测，逐股每手股数 + 港股费用模型 + 货币隔离
- 🧪 Rhai 嵌入式脚本策略引擎，回测避免未来函数（次日开盘成交）
- 💰 手续费/印花税建模，输出收益、回撤、夏普、胜率等绩效指标
- 📁 持仓管理与实时盈亏

## 安装

```bash
cargo install --path .
# 或
cargo build --release   # 产物在 target/release/stockrs
```

## 快速开始

```bash
# 1. 添加并下载股票日K数据
stockrs data add 000858 600519

# 2. 查看跟踪列表与数据范围
stockrs data list
stockrs data info 000858

# 3. 实时行情
stockrs quote 000858 600519
stockrs quote sh000001 上证指数 hs300   # 指数:交易所前缀 / 中文名 / 别名

# 4. 技术指标
stockrs indicator 000858 --period 20
stockrs indicator sh000001              # 指数指标(无需 data add,在线取数)

# 5. 回测策略
stockrs backtest strategies/sma_cross.rhai --stock 000858 --start 2023-01-01 --end 2025-01-01

# 6. 持仓管理
stockrs portfolio add 000858 --price 120 --quantity 500
stockrs portfolio list
stockrs portfolio history
```

## 命令一览

| 命令 | 说明 |
| --- | --- |
| `data add <code...>` | 添加股票并下载日K |
| `data update [code...]` | 增量更新日K（缺省更新全部） |
| `data remove <code>` | 移除跟踪 |
| `data list` / `data info <code>` | 查看列表 / 单只数据信息 |
| `quote <code...>` | 实时行情 |
| `indicator <code> [--period N]` | 最新技术指标 |
| `backtest <script> --stock <code> [--start --end --capital]` | 单标的回测 |
| `backtest <script> --stocks a,b,c` / `--universe` | 多股票组合回测 |
| `backtest <script> ... --benchmark hs300` | 叠加基准对比（收益/超额/Alpha/Beta） |
| `backtest <script> ... --param k=v1,v2 [--optimize sharpe]` | 参数扫描（网格寻优） |
| `strategy new <file>` | 生成带完整 ctx API 注释的策略模板（脚手架） |
| `portfolio add/remove/list/history` | 持仓管理 |
| `portfolio stats <code>` | 持仓收益分析（曲线/回撤/日均收益） |
| `self-update [--check]` | 更新 stockrs 自身到最新版本 |

> **指数代码说明**：指数与同码股票靠 6 位数字无法区分（`000001` 既是上证指数
> `1.000001`、也是平安银行 `0.000001`）。`quote` / `indicator` 里裸 `000001` 一律按**股票**
> 解析（＝平安银行）；要查指数请用**交易所前缀** `sh000001` / `sz399001`，或**别名/中文名**
> `上证指数` `sh` `hs300` `zz500` `cyb` `kc50` 等。`indicator` 查指数会在线取数、无需先 `data add`。

## 策略脚本

策略是一个 `.rhai` 文件，定义 `on_bar(ctx)`，每根 K 线调用一次。最快上手是让工具生成模板——
里面内联了完整 ctx API 注释和一个可直接跑的示例：

```bash
stockrs strategy new my.rhai
stockrs backtest my.rhai --stock 600519 --start 2023-01-01
```

```javascript
let name = "SMA Cross";

fn on_bar(ctx) {
    let ma5 = ctx.sma(5);
    let ma20 = ctx.sma(20);
    let prev_ma5 = ctx.sma_at(5, 1);
    let prev_ma20 = ctx.sma_at(20, 1);

    if prev_ma5 < prev_ma20 && ma5 > ma20 {
        ctx.buy(ctx.close, ctx.max_shares());   // 金叉买入
    }
    if prev_ma5 > prev_ma20 && ma5 < ma20 {
        ctx.sell(ctx.close, ctx.position());     // 死叉卖出
    }
}
```

### ctx API

| 分类 | 方法 |
| --- | --- |
| 行情 | `ctx.open` `ctx.high` `ctx.low` `ctx.close` `ctx.volume` `ctx.date` |
| 历史 | `ctx.close_at(n)` `ctx.sma_at(period, n)` |
| 指标 | `ctx.sma(p)` `ctx.ema(p)` `ctx.rsi(p)` `ctx.macd(f,s,sig)` `ctx.kdj(p)` `ctx.boll(p,mult)` |
| 账户 | `ctx.position()` `ctx.cash()` `ctx.total_value()` `ctx.max_shares()` |
| 基本面 | `ctx.pe` `ctx.pb` `ctx.ps` `ctx.mktcap`（按 bar 对齐，无数据 NaN） |
| 状态 | `ctx.set(key, v)` `ctx.get(key, default)` `ctx.has(key)`（跨 bar 持久，用于止盈止损等；Rhai 函数访问不了脚本全局变量） |
| 交易 | `ctx.buy(price, shares)` `ctx.sell(price, shares)` |

`macd` / `kdj` / `boll` 返回长度为 3 的数组。指标数据不足时返回 `NaN`，脚本可用 `x != x` 判断。

## 组合回测

多股票组合回测中，`on_bar(ctx)` 每个**交易日**调用一次，一次能看到整个股票池，
适合横截面选股与再平衡。行情/指标/交易方法都带一个 `code` 参数：

```bash
# 显式股票池
stockrs backtest strategies/momentum_rotation.rhai --stocks 600519,000858,300750
# 用全部已跟踪股票作为股票池
stockrs backtest strategies/momentum_rotation.rhai --universe
```

组合 ctx API：

| 分类 | 方法 |
| --- | --- |
| 选股 | `ctx.stocks()` 当日活跃代码 · `ctx.universe()` 全部 · `ctx.rank(lookback)` 按动量降序 |
| 行情 | `ctx.close(code)` `ctx.open/high/low/volume(code)` `ctx.close_at(code, n)` |
| 指标 | `ctx.sma(code, p)` `ctx.ema/rsi(code, p)` `ctx.macd(code,f,s,sig)` `ctx.kdj(code,p)` `ctx.boll(code,p,mult)` |
| 账户 | `ctx.position(code)` `ctx.avg_cost(code)` `ctx.cash()` `ctx.total_value()` `ctx.max_shares(code)` |
| 基本面 | `ctx.pe(code)` `ctx.pb(code)` `ctx.ps(code)` `ctx.mktcap(code)` |
| 状态 | `ctx.set(key, v)` `ctx.get(key, default)` `ctx.has(key)`（跨 bar 持久；组合可用 `"entry:"+code` 按股 key） |
| 交易 | `ctx.buy(code, shares)` `ctx.sell(code, shares)` `ctx.order_target_pct(code, pct)` 再平衡到目标权重 |

日期轴取股票池的**并集**，某股停牌当日按最近收盘估值；`rank` 只对当日活跃、
数据充足的股票排序（数据不足者剔除）。同一交易日的订单**先卖后买**，卖出回笼现金可供买入。

> 注意 rhai 区分整数与浮点：`order_target_pct` / `boll` 的浮点参数在脚本里要写小数
> （`0.5`、`2.0`），周期类参数写整数。

## 基本面数据（PE/PB）

`data add` / `data update` 会一并拉取历史日度估值,存入本地库并增量维护。A 股取自东财
datacenter(PE-TTM / PB-MRQ / PS-TTM / 总市值,约 8 年);港股取自百度股市通(PE-TTM /
PB / 总市值,近三年;港股无真市销率,`ps` 留空)。两者落进同一张表,下游对齐/回测逻辑完全一致。

- **回测里按 bar 无未来函数对齐**:在第 t 根 K 线只能读到第 t 天(或之前最近一天)的估值(on-or-before carry-forward),不会用未来数据。无数据处为 `NaN`,亏损股 PE 为负。
- 单标的策略用无参 `ctx.pe` / `ctx.pb` / `ctx.ps` / `ctx.mktcap`;组合策略用带 code 的 `ctx.pb(code)` 等。**A 股港股用法完全一致**。
- `indicator` 命令会显示最新 PE/PB/PS/总市值。

**双重验证示例**:技术面信号叠加基本面过滤——

```bash
# 单标的:金叉且 PB 低于阈值才买
stockrs backtest strategies/value_sma.rhai --stock 600519 --param pb_max=3,5,8 --optimize sharpe
# 港股同样可用(同一套 ctx.pe/ctx.pb)
stockrs backtest strategies/value_sma.rhai --stock hk00700 --param pb_max=3,5
# 组合:在低 PB 股票池里按动量轮动
stockrs backtest strategies/value_momentum.rhai --universe --param top_n=2,3 --param pb_max=3,5
```

## 基准对比

`--benchmark` 叠加同期指数买入持有对比，输出基准收益、超额收益、Beta、年化 Alpha。
支持别名 `hs300`(沪深300) `zz500`(中证500) `sh`(上证) `sz`(深证) `cyb`(创业板) `kc50`(科创50)：

```bash
stockrs backtest strategies/sma_cross.rhai --stock 600519 --benchmark hs300
```

基准数据先读本地库，缺失则直连东财并缓存；取不到时告警跳过，不影响回测本身。

## 参数扫描

策略用 `ctx.param(name, default)` 读取可注入参数（`default` 用整数则返回整数、
用小数则返回小数）。`--param key=v1,v2,...` 做网格（可多次指定，笛卡尔积，上限 200）：

```bash
stockrs backtest strategies/sma_cross_param.rhai --stock 600519 \
  --param fast=5,10 --param slow=20,30,60 --optimize sharpe
```

按 `--optimize`（`return` 默认 / `annual` / `sharpe` / `drawdown`）排序，输出各组合的
收益/年化/回撤/夏普/胜率表，`★` 标注最优行。组合回测同样支持扫描。

## 港股支持

港股代码用 `hk` 前缀、`.HK` 后缀,或直接写 5 位以内数字(A股/基金都是 6 位,无歧义):

```bash
stockrs data add hk00700          # 或 00700 / 700,自动补零到 5 位
stockrs quote 00700               # 腾讯控股港币行情(含 PE/PB)
stockrs indicator 00700           # 港股技术指标
stockrs backtest strategies/sma_cross.rhai --stock 00700 --start 2020-01-01
```

- **数据源**:腾讯为主(`fqkline` 前复权 + gtimg 行情),东财 secid 116 备选;新浪不覆盖港股。
- **回测算得对**:
  - **每手股数逐股不同**(00700=100、汇丰 00005=400、工行 01398=1000…),`data add` 时从东财 F10 取 `TRADE_UNIT` 落库,撮合按各自每手整手。
  - **港股费用模型**:印花税 0.1%(**买卖双向**,与 A 股仅卖出不同)+ 交易费/证监会征费/财汇局征费/结算费,券商佣金可配。A 股规则完全不变(零回归)。
  - **货币隔离**:港股按 HKD、A 股按 CNY;**同一组合回测不能混用**(会报错提示分开跑),单一市场 universe 正常。
- **基本面**:港股日频 PE/PB/总市值来自百度股市通(近三年),与 A 股同一套 PIT 对齐进回测,`ctx.pe` / `ctx.pb` / `ctx.mktcap` 港股一样可用(详见上文「基本面数据」章节);港股无真市销率,`ctx.ps` 留空。
- **货币符号**:港股回测报告显示 `HK$`,A 股显示 `¥`,按标的市场自动切换。

## 回测规则

- 初始资金默认 10 万，可用 `--capital` 覆盖
- 手续费：买入万三，卖出万三 + 千一印花税
- 最小交易单位 100 股（1 手），资金不足自动下调到可负担整手数
- **信号在当日收盘计算，次日开盘成交**，避免未来函数

## 数据来源

多源容灾，按顺序自动故障切换（某源超时/被封/改字段时切下一个），命令输出会标注实际来源：

| 优先级 | 数据源 | 行情 | 日K | PE/PB | 说明 |
| --- | --- | --- | --- | --- | --- |
| 1 | 东方财富 | ✅ | ✅ 前复权 | ✅ | 字段最全（含成交额、换手率），全历史区间；实时主机 push2 偶发不可达时切下一源 |
| 2 | 腾讯 `qt.gtimg.cn` | ✅ | ✅ 前复权 | ✅ A股+港股 | 支持区间，单次约 640 条 |
| 3 | 新浪 `hq.sinajs.cn` | ✅ | ⚠️ 非前复权 | ❌ | 日K仅最近约 1023 条，无成交额、无估值 |

**估值（PE/PB）的三层保障：** `quote` 的 PE/PB 依次取自 **东财实时 → 腾讯实时 → 本地基本面表**
（`fundamentals`，截至最近收盘，回退值标 `*` 并附脚注）。东财实时主机（push2）与基本面主机
（datacenter）相互独立，前者不可达时后两层仍能补上估值。各源具体字段下标（东财 f162/f167；
腾讯 A股 `[39]/[46]`、港股 `[57]/[58]`；港股 PE/PB 亦可由百度股市通日频提供）均为逆向所得，
注释在 `src/data/{eastmoney,tencent,hk}.rs`。

**礼貌爬取（避免把接口打挂）：**

- **批量优先**：`quote` 多只、`portfolio list` 刷新用批量接口（新浪 `list=`、腾讯 `q=`、东财 `ulist.np`），N 个请求压成 1 个，是最有效的一招
- **有界并发**：`data update` 多只时并发上限 4（`tokio::Semaphore`），只并行网络 IO，SQLite 写入仍串行
- **抖动 + 退避重试**：每请求前加 0~400ms 抖动打散齐射；失败按 300ms→600ms→1200ms 指数退避重试 3 次
- **增量更新**：只拉本地缺失的日期段，平时更新量很小

接口均无官方文档，字段由响应结构逆向得到（新浪/腾讯为 GBK 编码，价格缩放、字段顺序等关键点已在代码注释）。新增数据源只需实现 `data::source::{QuoteSource, KlineSource}` trait 并加入切换链。仅供研究学习，风险自负。

联网烟雾测试（默认忽略，不进 CI）：

```bash
cargo test -- --ignored
```

## 开发

```bash
cargo build      # 构建
cargo test       # 运行测试（指标 + 回测引擎）
```

## License

MIT © wnzzer
