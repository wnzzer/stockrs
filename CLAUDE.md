# stockrs — 项目设计提示词

## 给 Claude Code 的指令

请帮我创建一个名为 `stockrs` 的 Rust CLI 量化工具项目。以下是完整的设计要求。

## 项目定位

纯 Rust 编写的轻量 A 股量化 CLI 工具。编译为单二进制分发，零外部依赖，用户下载即用。项目自身负责数据的获取和维护，用户只需编写策略脚本，CLI 负责回测执行和结果输出。

**核心理念：** 逻辑在代码里，不在 LLM 里。所有计算、回测、信号判断都是确定性的代码，同样的输入永远产生同样的输出。

## 技术栈

- 语言：Rust（edition 2021）
- 数据存储：SQLite（rusqlite），单文件数据库，零部署
- HTTP 请求：reqwest（tokio 异步）
- 策略脚本引擎：rhai（纯 Rust 嵌入式脚本语言）
- CLI 框架：clap（derive 模式）
- 序列化：serde + serde_json
- 表格输出：tabled 或 comfy-table
- 异步运行时：tokio

## 数据源

对接东方财富公开 HTTP API（无需 API Key），主要接口：

1. **日K线数据**：`https://push2his.eastmoney.com/api/qt/stock/kline/get`
   - 参数：secid, fields1, fields2, klt(101=日K), fqt(1=前复权), beg, end
   - secid 格式：沪市 `1.600000`，深市 `0.000001`，创业板 `0.300001`

2. **实时行情**：`https://push2.eastmoney.com/api/qt/stock/get`

3. **股票列表**：`https://push2.eastmoney.com/api/qt/clist/get`

4. **板块/概念**：按需后续添加

注意：东财接口没有官方文档，需要从响应结构逆向解析。请在代码中对 API 响应格式做好注释和错误处理。

## 项目结构

```
stockrs/
├── Cargo.toml
├── README.md
├── LICENSE                  # MIT
├── .gitignore
├── src/
│   ├── main.rs             # CLI 入口，clap 命令路由
│   ├── cli.rs              # 命令枚举 + 路由（模块根，Rust 2018 风格，不用 mod.rs）
│   ├── cli/
│   │   ├── data.rs         # data 子命令
│   │   ├── quote.rs        # quote 子命令
│   │   ├── backtest.rs     # backtest 子命令
│   │   ├── portfolio.rs    # portfolio 子命令
│   │   └── indicator.rs    # indicator 子命令
│   ├── data.rs             # 数据层模块根
│   ├── data/
│   │   ├── eastmoney.rs    # 东财 API 请求/解析
│   │   ├── store.rs        # SQLite 存储层
│   │   └── models.rs       # 数据模型（KLine, Stock 等）
│   ├── engine.rs           # 回测引擎模块根
│   ├── engine/
│   │   ├── backtest.rs     # 回测引擎核心
│   │   ├── context.rs      # 策略上下文（持仓、资金、订单）
│   │   └── metrics.rs      # 绩效指标计算
│   ├── indicator.rs        # 指标模块根
│   ├── indicator/
│   │   ├── ma.rs           # MA / EMA / SMA
│   │   ├── rsi.rs          # RSI
│   │   ├── macd.rs         # MACD
│   │   ├── kdj.rs          # KDJ
│   │   └── boll.rs         # 布林带
│   ├── strategy.rs         # 策略模块根
│   ├── strategy/
│   │   └── rhai_engine.rs  # Rhai 脚本引擎，注册函数和绑定
│   ├── utils.rs            # 工具模块根
│   └── utils/
│       └── format.rs       # 输出格式化
└── strategies/              # 示例策略目录
    ├── sma_cross.rhai       # 均线交叉策略
    └── rsi_oversold.rhai    # RSI 超卖策略
```

## CLI 命令设计

```bash
# 数据管理
stockrs data update                    # 增量更新所有已跟踪股票的日K数据
stockrs data update 600519            # 更新指定股票
stockrs data add 600519 000858        # 添加股票到跟踪列表
stockrs data remove 600519            # 移除跟踪
stockrs data list                      # 查看已跟踪的股票列表
stockrs data info 600519              # 查看某只股票的数据范围和条数

# 实时行情
stockrs quote 600519                   # 查看实时行情
stockrs quote 600519 000858 300750    # 批量查看

# 技术指标
stockrs indicator 600519              # 显示最新技术指标（MA/RSI/MACD/KDJ/BOLL）
stockrs indicator 600519 --period 20  # 指定周期

# 回测
stockrs backtest strategies/sma_cross.rhai --stock 600519 --start 2024-01-01 --end 2025-01-01
stockrs backtest strategies/sma_cross.rhai --stock 600519 --capital 100000

# 持仓管理
stockrs portfolio add 600519 --price 1800 --quantity 100 --date 2025-01-15
stockrs portfolio remove 600519
stockrs portfolio list                 # 当前持仓 + 实时盈亏
stockrs portfolio history              # 历史交易记录
```

## 数据库 Schema

```sql
-- 股票基本信息
CREATE TABLE stocks (
    code TEXT PRIMARY KEY,       -- 股票代码 如 "600519"
    name TEXT NOT NULL,          -- 股票名称
    market TEXT NOT NULL,        -- "SH" / "SZ"
    added_at TEXT NOT NULL       -- 添加时间
);

-- 日K线数据
CREATE TABLE klines (
    code TEXT NOT NULL,
    date TEXT NOT NULL,           -- "2025-01-15"
    open REAL NOT NULL,
    high REAL NOT NULL,
    low REAL NOT NULL,
    close REAL NOT NULL,
    volume REAL NOT NULL,         -- 成交量（手）
    amount REAL NOT NULL,         -- 成交额（元）
    turnover REAL,               -- 换手率
    PRIMARY KEY (code, date)
);

-- 持仓
CREATE TABLE portfolio (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    code TEXT NOT NULL,
    price REAL NOT NULL,          -- 买入价
    quantity INTEGER NOT NULL,    -- 数量（股）
    date TEXT NOT NULL,           -- 买入日期
    note TEXT                     -- 备注
);

-- 交易记录
CREATE TABLE trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    code TEXT NOT NULL,
    action TEXT NOT NULL,          -- "buy" / "sell"
    price REAL NOT NULL,
    quantity INTEGER NOT NULL,
    date TEXT NOT NULL,
    note TEXT
);
```

## Rhai 策略脚本接口

策略脚本通过 Rhai 编写，引擎注册以下函数供策略调用：

```javascript
// 示例策略：双均线交叉
// strategies/sma_cross.rhai

// 策略元信息
let name = "SMA Cross";
let description = "5日均线上穿20日均线买入，下穿卖出";

// on_bar 函数：每根K线调用一次
fn on_bar(ctx) {
    let ma5 = ctx.sma(5);      // 5日均线
    let ma20 = ctx.sma(20);    // 20日均线
    let prev_ma5 = ctx.sma_at(5, 1);   // 前一天的5日均线
    let prev_ma20 = ctx.sma_at(20, 1);

    // 金叉买入
    if prev_ma5 < prev_ma20 && ma5 > ma20 {
        ctx.buy(ctx.close, ctx.max_shares());
    }

    // 死叉卖出
    if prev_ma5 > prev_ma20 && ma5 < ma20 {
        ctx.sell(ctx.close, ctx.position());
    }
}
```

**ctx 对象提供的方法：**

```
// 行情数据
ctx.open, ctx.high, ctx.low, ctx.close, ctx.volume
ctx.date                          // 当前日期

// 技术指标
ctx.sma(period)                   // 简单移动平均
ctx.ema(period)                   // 指数移动平均
ctx.rsi(period)                   // RSI
ctx.macd(fast, slow, signal)      // 返回 [dif, dea, macd]
ctx.kdj(period)                   // 返回 [k, d, j]
ctx.boll(period, multiplier)      // 返回 [upper, mid, lower]

// 历史数据
ctx.close_at(n)                   // n天前的收盘价
ctx.sma_at(period, n)             // n天前的SMA值

// 交易操作
ctx.buy(price, shares)            // 买入
ctx.sell(price, shares)           // 卖出
ctx.position()                    // 当前持仓数量
ctx.cash()                        // 可用资金
ctx.max_shares()                  // 当前价格下最大可买数量（考虑手续费）
ctx.total_value()                 // 总资产（现金+持仓市值）
```

## 回测输出格式

```
╔══════════════════════════════════════════════╗
║          Backtest Report: SMA Cross          ║
╠══════════════════════════════════════════════╣
║ Stock:          600519 贵州茅台              ║
║ Period:         2024-01-01 ~ 2025-01-01      ║
║ Initial:        ¥100,000.00                  ║
║ Final:          ¥112,350.00                  ║
║ Return:         +12.35%                      ║
║ Annual Return:  +12.35%                      ║
║ Max Drawdown:   -8.23%                       ║
║ Sharpe Ratio:   1.45                         ║
║ Win Rate:       62.5% (5/8)                  ║
║ Total Trades:   8                            ║
╠══════════════════════════════════════════════╣
║ Trades:                                      ║
║ 2024-02-15  BUY   100 @ ¥1,680.00           ║
║ 2024-03-20  SELL  100 @ ¥1,750.00  +4.17%   ║
║ ...                                          ║
╚══════════════════════════════════════════════╝
```

## 回测引擎规则

- 初始资金默认 10 万
- 手续费：买入万三，卖出万三 + 千一印花税
- 最小交易单位：100 股（1 手）
- 滑点：暂不考虑（日K级别影响小）
- 信号触发：当天收盘价计算信号，次日开盘价成交（避免未来函数）

## 开发顺序

请按以下顺序逐步实现，每步确保编译通过 + 可运行：

### Phase 1：骨架 + 数据层
1. `cargo init stockrs` 初始化项目
2. 搭建 CLI 框架（clap），所有子命令先占位
3. 实现 SQLite 存储层（建表、CRUD）
4. 对接东财日K线 API，实现 `data add` + `data update`
5. 实现 `data list` / `data info`

### Phase 2：行情 + 指标
6. 实现 `quote` 实时行情查询
7. 实现技术指标计算（MA/RSI/MACD/KDJ/BOLL）
8. 实现 `indicator` 命令

### Phase 3：回测引擎
9. 实现回测引擎核心（资金管理、订单撮合、绩效统计）
10. 集成 Rhai 脚本引擎，注册 ctx 方法
11. 实现 `backtest` 命令
12. 编写示例策略

### Phase 4：持仓管理
13. 实现 `portfolio` 相关命令
14. 持仓盈亏计算（结合实时行情）

## 代码风格

- 错误处理统一用 `anyhow`
- 模块间传递数据用结构体，不用裸 tuple
- 保持函数短小，单一职责
- 不写注释，除非有隐藏陷阱（如东财接口的坑）
- 测试覆盖核心逻辑（指标计算、回测引擎）

## 参考

- 作者 GitHub: wnzzer，有 rank-analysis（Tauri 2 + Rust）项目经验
- Rust 水平：中级偏上，熟悉 tokio/Arc/Mutex/模块化
- 目标：做到 rank-analysis 同等工程质量，README 完善，CI 完善
