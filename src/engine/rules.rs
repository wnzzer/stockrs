//! 市场交易规则:每手股数取整 + 费用模型。A股与港股规则不同,回测按标的市场选档。

use crate::data::Market;

/// 每手股数向下取整(整手撮合)。lot<=0 视为不取整。lot=100 时与旧的 (x/100)*100 逐位一致。
pub fn floor_to_lot(shares: i64, lot: i64) -> i64 {
    if lot <= 0 {
        shares
    } else {
        (shares / lot) * lot
    }
}

/// 一笔交易的费用模型。
#[derive(Clone)]
pub enum FeeModel {
    /// A股:买入 buy_rate,卖出 sell_rate + 印花税 stamp_rate(仅卖出)。
    AShare {
        buy_rate: f64,
        sell_rate: f64,
        stamp_rate: f64,
    },
    /// 港股:官方分项 + 券商佣金(可配)。买卖双向都收(含印花)。
    Hk(HkFees),
}

/// 港股费用档(现行官方固定档 + 券商可变佣金)。费率来源 HKEX 官方费用页。
#[derive(Clone)]
pub struct HkFees {
    pub stamp_rate: f64,       // 印花税 0.1%(向上取整到整 HK$)
    pub trading_fee_rate: f64, // 联交所交易费 0.00565%
    pub sfc_rate: f64,         // 证监会交易征费 0.0027%
    pub afrc_rate: f64,        // 财汇局征费 0.00015%
    pub settlement_rate: f64,  // 结算费 0.0042%(现行,无最低/最高)
    pub settlement_min: f64,   // 0 = 不限
    pub settlement_max: f64,   // 0 = 不限
    pub commission_rate: f64,  // 券商佣金率(可配)
    pub commission_min: f64,   // 最低佣金(可配)
    pub platform_fee: f64,     // 平台费/笔(可配)
}

impl HkFees {
    /// 现行官方档 + 互联网券商(零佣金)预设。
    pub fn retail() -> HkFees {
        HkFees {
            stamp_rate: 0.001,
            trading_fee_rate: 0.0000565,
            sfc_rate: 0.000027,
            afrc_rate: 0.0000015,
            settlement_rate: 0.000042,
            settlement_min: 0.0,
            settlement_max: 0.0,
            commission_rate: 0.0,
            commission_min: 0.0,
            platform_fee: 0.0,
        }
    }

    /// 单笔(买或卖,港股对称)总费用。
    fn cost(&self, notional: f64) -> f64 {
        let stamp = (notional * self.stamp_rate).ceil(); // 印花税向上取整到整 HK$
        let trading = notional * self.trading_fee_rate;
        let sfc = notional * self.sfc_rate;
        let afrc = notional * self.afrc_rate;
        let mut settlement = notional * self.settlement_rate;
        if self.settlement_min > 0.0 {
            settlement = settlement.max(self.settlement_min);
        }
        if self.settlement_max > 0.0 {
            settlement = settlement.min(self.settlement_max);
        }
        let commission =
            (notional * self.commission_rate).max(self.commission_min) + self.platform_fee;
        stamp + trading + sfc + afrc + settlement + commission
    }

    /// 近似买入侧费率(供 max_shares 估算可买量,忽略最低收费)。
    fn approx_rate(&self) -> f64 {
        self.stamp_rate
            + self.trading_fee_rate
            + self.sfc_rate
            + self.afrc_rate
            + self.settlement_rate
            + self.commission_rate
    }
}

impl FeeModel {
    /// A股档(与旧 Fees::default() 逐位相等,保证零回归)。
    pub fn a_share() -> FeeModel {
        FeeModel::AShare {
            buy_rate: 0.0003,
            sell_rate: 0.0003,
            stamp_rate: 0.001,
        }
    }

    pub fn hk() -> FeeModel {
        FeeModel::Hk(HkFees::retail())
    }

    pub fn for_market(m: Market) -> FeeModel {
        match m {
            Market::HK => FeeModel::hk(),
            _ => FeeModel::a_share(),
        }
    }

    /// 买入一笔的费用(货币单位同标的)。
    pub fn buy_cost(&self, notional: f64) -> f64 {
        match self {
            FeeModel::AShare { buy_rate, .. } => notional * buy_rate,
            FeeModel::Hk(h) => h.cost(notional),
        }
    }

    /// 卖出一笔的费用。
    pub fn sell_cost(&self, notional: f64) -> f64 {
        match self {
            FeeModel::AShare {
                sell_rate,
                stamp_rate,
                ..
            } => notional * (sell_rate + stamp_rate),
            FeeModel::Hk(h) => h.cost(notional),
        }
    }

    /// max_shares 估算用的买入侧近似费率。
    pub fn buy_rate_approx(&self) -> f64 {
        match self {
            FeeModel::AShare { buy_rate, .. } => *buy_rate,
            FeeModel::Hk(h) => h.approx_rate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_to_lot_basic() {
        assert_eq!(floor_to_lot(1234, 100), 1200);
        assert_eq!(floor_to_lot(1234, 500), 1000);
        assert_eq!(floor_to_lot(999, 1000), 0);
        assert_eq!(floor_to_lot(1234, 0), 1234); // 不取整
    }

    #[test]
    fn a_share_fees_match_old_constants() {
        // 旧逻辑:买 cost*0.0003,卖 proceeds*(0.0003+0.001)
        let f = FeeModel::a_share();
        assert!((f.buy_cost(100_000.0) - 30.0).abs() < 1e-9);
        assert!((f.sell_cost(100_000.0) - 130.0).abs() < 1e-9);
        assert!((f.buy_rate_approx() - 0.0003).abs() < 1e-12);
    }

    #[test]
    fn hk_fees_charge_both_directions() {
        let f = FeeModel::hk();
        // 官方档双边合计约 0.225%(不含佣金);买卖对称
        let buy = f.buy_cost(100_000.0);
        let sell = f.sell_cost(100_000.0);
        assert!((buy - sell).abs() < 1e-9, "港股买卖费用应对称");
        assert!(buy > 100.0 && buy < 130.0, "buy={}", buy); // 约 112.5 + 印花取整
    }
}
