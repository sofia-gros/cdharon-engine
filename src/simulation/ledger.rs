//! material IDごとのcell token発行・消費累計を保持し、World人口との対応を監査します。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialBalance {
    pub issued: u64,
    pub consumed: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialDelta {
    pub material_id: u16,
    pub issue: u64,
    pub consume: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialLedger {
    balances: Vec<MaterialBalance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerError {
    AllocationFailed,
    InvalidMaterial,
    InvalidDeltaOrder,
    CounterOverflow,
    Unbalanced,
}

impl MaterialLedger {
    pub fn try_new(material_count: usize) -> Result<Self, LedgerError> {
        let mut balances = Vec::new();
        balances
            .try_reserve_exact(material_count)
            .map_err(|_| LedgerError::AllocationFailed)?;
        balances.resize(material_count, MaterialBalance::default());
        Ok(Self { balances })
    }

    pub fn balance(&self, material_id: u16) -> Option<MaterialBalance> {
        self.balances.get(material_id as usize).copied()
    }

    pub fn balances(&self) -> &[MaterialBalance] {
        &self.balances
    }

    /// 既存残高を維持した候補台帳を、確保失敗を呼び出し元へ返せる形で複製します。
    /// チャンク公開前の物質発行を候補上で検証し、セル確保が失敗した際に
    /// 正規台帳だけが先に進んでしまう部分commitを防ぐために使用します。
    pub fn try_clone(&self) -> Result<Self, LedgerError> {
        let mut balances = Vec::new();
        balances
            .try_reserve_exact(self.balances.len())
            .map_err(|_| LedgerError::AllocationFailed)?;
        balances.extend_from_slice(&self.balances);
        Ok(Self { balances })
    }

    #[cfg(test)]
    pub(crate) fn set_balance_for_test(&mut self, material_id: u16, balance: MaterialBalance) {
        self.balances[material_id as usize] = balance;
    }

    /// 全deltaのID順・残高・整数上限を先に検証し、全て妥当な場合だけまとめて適用します。
    /// IDは昇順かつ重複なしでなければならず、途中要素の失敗で台帳を部分更新しません。
    pub fn apply(&mut self, deltas: &[MaterialDelta]) -> Result<(), LedgerError> {
        let mut previous = None;
        for delta in deltas {
            if delta.material_id as usize >= self.balances.len() {
                return Err(LedgerError::InvalidMaterial);
            }
            if previous.is_some_and(|id| id >= delta.material_id) {
                return Err(LedgerError::InvalidDeltaOrder);
            }
            previous = Some(delta.material_id);
            let balance = self.balances[delta.material_id as usize];
            let issued = balance
                .issued
                .checked_add(delta.issue)
                .ok_or(LedgerError::CounterOverflow)?;
            let consumed = balance
                .consumed
                .checked_add(delta.consume)
                .ok_or(LedgerError::CounterOverflow)?;
            if consumed > issued {
                return Err(LedgerError::Unbalanced);
            }
        }
        for delta in deltas {
            let balance = &mut self.balances[delta.material_id as usize];
            balance.issued += delta.issue;
            balance.consumed += delta.consume;
        }
        Ok(())
    }

    pub fn reset_to_population(&mut self, counts: &[u64]) -> Result<(), LedgerError> {
        if counts.len() != self.balances.len() {
            return Err(LedgerError::InvalidMaterial);
        }
        for (balance, count) in self.balances.iter_mut().zip(counts) {
            *balance = MaterialBalance {
                issued: *count,
                consumed: 0,
            };
        }
        Ok(())
    }

    pub(crate) fn clear_for_rebuild(&mut self) {
        self.balances.fill(MaterialBalance::default());
    }

    pub(crate) fn issue_genesis(&mut self, material_id: u16) -> Result<(), LedgerError> {
        let balance = self
            .balances
            .get_mut(material_id as usize)
            .ok_or(LedgerError::InvalidMaterial)?;
        balance.issued = balance
            .issued
            .checked_add(1)
            .ok_or(LedgerError::CounterOverflow)?;
        Ok(())
    }

    pub fn validate_population(&self, counts: &[u64]) -> Result<(), LedgerError> {
        if counts.len() != self.balances.len() {
            return Err(LedgerError::InvalidMaterial);
        }
        for (balance, count) in self.balances.iter().zip(counts) {
            if balance.issued.checked_sub(balance.consumed) != Some(*count) {
                return Err(LedgerError::Unbalanced);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_is_atomic_on_overflow_or_bad_order() {
        let mut ledger = MaterialLedger::try_new(3).unwrap();
        ledger
            .apply(&[
                MaterialDelta {
                    material_id: 1,
                    issue: 4,
                    consume: 0,
                },
                MaterialDelta {
                    material_id: 2,
                    issue: 2,
                    consume: 1,
                },
            ])
            .unwrap();
        let before = ledger.clone();
        assert_eq!(
            ledger.apply(&[
                MaterialDelta {
                    material_id: 0,
                    issue: 1,
                    consume: 0,
                },
                MaterialDelta {
                    material_id: 1,
                    issue: u64::MAX,
                    consume: 0,
                },
            ]),
            Err(LedgerError::CounterOverflow)
        );
        assert_eq!(ledger, before);
        assert_eq!(
            ledger.apply(&[
                MaterialDelta {
                    material_id: 2,
                    issue: 1,
                    consume: 0,
                },
                MaterialDelta {
                    material_id: 1,
                    issue: 1,
                    consume: 0,
                },
            ]),
            Err(LedgerError::InvalidDeltaOrder)
        );
        assert_eq!(ledger, before);
    }

    #[test]
    fn balance_matches_population_after_explicit_conversion() {
        let mut ledger = MaterialLedger::try_new(3).unwrap();
        ledger.reset_to_population(&[0, 10, 0]).unwrap();
        ledger
            .apply(&[
                MaterialDelta {
                    material_id: 1,
                    issue: 0,
                    consume: 3,
                },
                MaterialDelta {
                    material_id: 2,
                    issue: 3,
                    consume: 0,
                },
            ])
            .unwrap();
        ledger.validate_population(&[0, 7, 3]).unwrap();
        assert_eq!(
            ledger.validate_population(&[0, 8, 2]),
            Err(LedgerError::Unbalanced)
        );
    }
}
