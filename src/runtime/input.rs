//! Scene scopeへ属する入力アクションと、tickへ割り当てた機器イベント。
//!
//! 物理キーをSceneの速度等へ直接結び付けません。複数bindingのORを評価し、
//! 同tick内の押下・解放も失わず、OSのキーrepeatでedgeを重複させません。

use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    id::{Arena, Id},
};

/// Sceneの実行期間を区別する型です。
pub enum ScopeKind {}
/// 停止・再開始で世代が変わるscope識別子です。
pub type ScopeId = Id<ScopeKind>;
/// 入力登録を区別する型です。
pub enum ActionKind {}
/// コピーしても登録の寿命を延ばさないアクションIDです。
pub type ActionId = Id<ActionKind>;

/// backendに依存しない入力の所在です。keyboard名はkey-a、arrow-left等を使います。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputBinding {
    /// 物理キーボードのキー。
    Keyboard(StableKey),
    /// mouse button番号。
    MouseButton(u32),
    /// local playerとgamepadのcontrol。
    Gamepad {
        /// 対象player。
        player: u16,
        /// controlの安定名。
        control: StableKey,
    },
    /// mouseやtouchの座標。
    Pointer(u16),
    /// 作者の機器adapterが定義するcontrol。
    Custom(StableKey),
}

/// 機器eventの値です。非有限値や範囲外axisは取込前に拒否します。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputValue {
    /// 押下中かどうか。
    Button(bool),
    /// -1から1のaxis。
    Axis(f32),
    /// 論理RenderFrame座標。cameraごとのWorld座標へは作者が明示変換します。
    Pointer {
        /// 水平方向。
        x: f64,
        /// 垂直方向。
        y: f64,
    },
    /// pointerがframe外・余白・window外にあり、有効な論理座標を持ちません。
    /// 最後の座標を残すと、画面外へ出てもゲーム内の同じ位置を指し続けるため明示します。
    PointerUnavailable,
}

/// 同一source内で順序を固定する機器eventです。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputEvent {
    /// 機器adapterのsource番号。
    pub source: u32,
    /// source内のsequence。
    pub sequence: u64,
    /// 操作したcontrol。
    pub binding: InputBinding,
    /// controlの新しい値。
    pub value: InputValue,
}

/// inputを対象tickへ明示して渡します。headlessとwindow hostで共通です。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickInput {
    /// 次に確定しようとするtick。初回は1です。
    pub tick: u64,
    /// source/sequence順へ正規化するevent。
    pub events: Vec<InputEvent>,
    /// focus喪失・機器切断時に全controlを中立へ戻します。
    pub reset: bool,
}
impl TickInput {
    /// 入力変化がないtickを構築します。held状態は維持します。
    pub fn empty(tick: u64) -> Self {
        Self {
            tick,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Button,
    Axis,
    Pointer,
}
struct Action {
    scope: ScopeId,
    key: StableKey,
    bindings: Vec<InputBinding>,
    kind: ValueKind,
    pressed: bool,
    released: bool,
}

pub(crate) struct InputManager {
    actions: Arena<Action, ActionKind>,
    devices: Vec<(InputBinding, InputValue)>,
    control_limit: u32,
    sequences: Vec<(u32, u64)>,
}

impl InputManager {
    pub(crate) fn new(capacity: u32) -> EngineResult<Self> {
        Ok(Self {
            actions: Arena::new(capacity)?,
            devices: Vec::new(),
            control_limit: capacity
                .checked_mul(32)
                .ok_or_else(|| invalid("入力control容量のoverflow"))?,
            sequences: Vec::new(),
        })
    }
    pub(crate) fn release_scope(&mut self, scope: ScopeId) -> EngineResult<()> {
        loop {
            let id = self
                .actions
                .iter()
                .find(|(_, action)| action.scope == scope)
                .map(|(id, _)| id);
            let Some(id) = id else {
                break;
            };
            self.actions.remove(id)?;
        }
        Ok(())
    }
    pub(crate) fn apply(&mut self, input: &TickInput) -> EngineResult<()> {
        // 全eventを検証してから機器stateを変更します。
        for event in &input.events {
            validate(event.binding, event.value)?;
        }
        if input
            .events
            .windows(2)
            .any(|pair| (pair[0].source, pair[0].sequence) >= (pair[1].source, pair[1].sequence))
        {
            return Err(invalid("入力sequenceの重複または逆順"));
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(self.actions.len())
            .map_err(crate::foundation::error::allocation)?;
        ids.extend(self.actions.iter().map(|(id, _)| id));
        let mut devices = Vec::new();
        devices
            .try_reserve_exact(self.devices.len())
            .map_err(crate::foundation::error::allocation)?;
        devices.extend_from_slice(&self.devices);
        let mut sequences = Vec::new();
        sequences
            .try_reserve_exact(self.sequences.len())
            .map_err(crate::foundation::error::allocation)?;
        sequences.extend_from_slice(&self.sequences);
        let mut edges = Vec::new();
        edges
            .try_reserve_exact(ids.len())
            .map_err(crate::foundation::error::allocation)?;
        edges.resize(ids.len(), (false, false));
        let mut before = Vec::new();
        before
            .try_reserve_exact(ids.len())
            .map_err(crate::foundation::error::allocation)?;
        if input.reset {
            for (index, id) in ids.iter().enumerate() {
                let was = button(self.actions.get(*id)?, &self.devices);
                edges[index].1 = was;
            }
            devices.clear();
        }
        for event in &input.events {
            let sequence = sequences.binary_search_by_key(&event.source, |(source, _)| *source);
            match sequence {
                Ok(index) if sequences[index].1 >= event.sequence => {
                    return Err(invalid("入力sequenceの再送または巻戻し"));
                }
                Ok(index) => sequences[index].1 = event.sequence,
                Err(index) => {
                    if sequences.len() >= self.control_limit as usize {
                        return Err(invalid("入力source数の上限"));
                    }
                    sequences
                        .try_reserve(1)
                        .map_err(crate::foundation::error::allocation)?;
                    sequences.insert(index, (event.source, event.sequence));
                }
            }
            before.clear();
            for id in &ids {
                let action = self.actions.get(*id)?;
                let kind = match event.value {
                    InputValue::Button(_) => ValueKind::Button,
                    InputValue::Axis(_) => ValueKind::Axis,
                    InputValue::Pointer { .. } | InputValue::PointerUnavailable => {
                        ValueKind::Pointer
                    }
                };
                if action.bindings.contains(&event.binding) && action.kind != kind {
                    return Err(invalid("入力eventと登録アクションの型が異なる"));
                }
                before.push(button(action, &devices));
            }
            let index = devices.binary_search_by_key(&event.binding, |(binding, _)| *binding);
            match index {
                Ok(index) => devices[index].1 = event.value,
                Err(index) => {
                    if devices.len() >= self.control_limit as usize {
                        return Err(EngineError::capacity(
                            "入力control数",
                            devices.len() as u64 + 1,
                            u64::from(self.control_limit),
                        ));
                    }
                    devices
                        .try_reserve(1)
                        .map_err(|_| invalid("入力bufferの確保"))?;
                    devices.insert(index, (event.binding, event.value));
                }
            }
            for (index, id) in ids.iter().enumerate() {
                let after = button(self.actions.get(*id)?, &devices);
                edges[index].0 |= !before[index] && after;
                edges[index].1 |= before[index] && !after;
            }
        }
        for (id, (pressed, released)) in ids.into_iter().zip(edges) {
            let action = self.actions.get_mut(id)?;
            action.pressed = pressed;
            action.released = released;
        }
        self.devices = devices;
        self.sequences = sequences;
        Ok(())
    }
    pub(crate) fn access(&mut self, scope: ScopeId, enabled: bool) -> InputAccess<'_> {
        InputAccess {
            manager: self,
            scope,
            enabled,
        }
    }
}

/// 現在のScene scopeだけが持つ登録・読取権限です。
pub struct InputAccess<'a> {
    manager: &'a mut InputManager,
    scope: ScopeId,
    enabled: bool,
}
impl InputAccess<'_> {
    /// buttonアクションの構築を始めます。
    pub fn action(&mut self, key: &str) -> ActionBuilder<'_> {
        ActionBuilder {
            manager: self.manager,
            scope: self.scope,
            key: StableKey::resolve("game", key),
            bindings: Vec::new(),
            kind: ValueKind::Button,
        }
    }
    /// axisアクションの構築を始めます。
    pub fn axis_action(&mut self, key: &str) -> ActionBuilder<'_> {
        let mut builder = self.action(key);
        builder.kind = ValueKind::Axis;
        builder
    }
    /// pointerアクションの構築を始めます。
    pub fn pointer_action(&mut self, key: &str) -> ActionBuilder<'_> {
        let mut builder = self.action(key);
        builder.kind = ValueKind::Pointer;
        builder
    }
    /// 複数bindingのいずれかが押されているかを返します。
    pub fn held(&self, id: ActionId) -> EngineResult<bool> {
        let action = self.checked(id, ValueKind::Button)?;
        Ok(self.enabled && button(action, &self.manager.devices))
    }
    /// このtickに押下edgeがあったかを返します。
    pub fn just_pressed(&self, id: ActionId) -> EngineResult<bool> {
        let action = self.checked(id, ValueKind::Button)?;
        Ok(self.enabled && action.pressed)
    }
    /// このtickに解放edgeがあったかを返します。
    pub fn just_released(&self, id: ActionId) -> EngineResult<bool> {
        let action = self.checked(id, ValueKind::Button)?;
        Ok(self.enabled && action.released)
    }
    /// binding順で最後の非ゼロaxisを取得します。
    pub fn axis(&self, id: ActionId) -> EngineResult<f32> {
        let action = self.checked(id, ValueKind::Axis)?;
        if !self.enabled {
            return Ok(0.0);
        }
        Ok(action
            .bindings
            .iter()
            .filter_map(|binding| match device(&self.manager.devices, *binding) {
                Some(InputValue::Axis(value)) if value != 0.0 => Some(value),
                _ => None,
            })
            .next_back()
            .unwrap_or(0.0))
    }
    /// pointerが利用可能なら座標を返します。
    pub fn pointer(&self, id: ActionId) -> EngineResult<Option<(f64, f64)>> {
        let action = self.checked(id, ValueKind::Pointer)?;
        if !self.enabled {
            return Ok(None);
        }
        Ok(action.bindings.iter().find_map(|binding| {
            match device(&self.manager.devices, *binding) {
                Some(InputValue::Pointer { x, y }) => Some((x, y)),
                _ => None,
            }
        }))
    }
    fn checked(&self, id: ActionId, kind: ValueKind) -> EngineResult<&Action> {
        let action = self.manager.actions.get(id)?;
        if action.scope != self.scope {
            return Err(EngineError::new(
                ErrorCode::WrongScope,
                "入力アクションのScene所属",
            ));
        }
        if action.kind != kind {
            return Err(invalid("入力アクションの値型"));
        }
        Ok(action)
    }
}

/// アクションを組み立てます。registerまで登録は行いません。
pub struct ActionBuilder<'a> {
    manager: &'a mut InputManager,
    scope: ScopeId,
    key: EngineResult<StableKey>,
    bindings: Vec<InputBinding>,
    kind: ValueKind,
}
impl ActionBuilder<'_> {
    /// 物理キー名を追加します。
    pub fn key(mut self, key: &str) -> Self {
        match StableKey::resolve("keyboard", key) {
            Ok(key) => self.bindings.push(InputBinding::Keyboard(key)),
            Err(error) => self.key = Err(error),
        };
        self
    }
    /// gamepadのcontrolを追加します。
    pub fn gamepad(mut self, player: u16, control: &str) -> Self {
        match StableKey::resolve("gamepad", control) {
            Ok(control) => self
                .bindings
                .push(InputBinding::Gamepad { player, control }),
            Err(error) => self.key = Err(error),
        };
        self
    }
    /// 独自機器やpointer等のbindingを追加します。
    pub fn binding(mut self, binding: InputBinding) -> Self {
        self.bindings.push(binding);
        self
    }
    /// scope内の重複とbinding型を検証して登録します。
    pub fn register(self) -> EngineResult<ActionId> {
        let key = self.key?;
        if self.bindings.is_empty() || self.bindings.len() > 32 {
            return Err(invalid("アクションbinding数は1から32"));
        }
        if self
            .manager
            .actions
            .iter()
            .any(|(_, action)| action.scope == self.scope && action.key == key)
        {
            return Err(EngineError::new(
                ErrorCode::DuplicateKey,
                "Scene内の入力アクション",
            ));
        }
        for binding in &self.bindings {
            if matches!(
                binding,
                InputBinding::Keyboard(_) | InputBinding::MouseButton(_)
            ) && self.kind != ValueKind::Button
            {
                return Err(invalid("button機器へaxis等を割り当てた"));
            }
            if matches!(binding, InputBinding::Pointer(_)) && self.kind != ValueKind::Pointer {
                return Err(invalid("pointerの値型"));
            }
        }
        self.manager.actions.insert(Action {
            scope: self.scope,
            key,
            bindings: self.bindings,
            kind: self.kind,
            pressed: false,
            released: false,
        })
    }
}

fn device(devices: &[(InputBinding, InputValue)], binding: InputBinding) -> Option<InputValue> {
    devices
        .binary_search_by_key(&binding, |(key, _)| *key)
        .ok()
        .map(|index| devices[index].1)
}
fn button(action: &Action, devices: &[(InputBinding, InputValue)]) -> bool {
    action.kind == ValueKind::Button
        && action
            .bindings
            .iter()
            .any(|binding| device(devices, *binding) == Some(InputValue::Button(true)))
}
fn validate(binding: InputBinding, value: InputValue) -> EngineResult<()> {
    match value {
        InputValue::Axis(value) if !value.is_finite() || !(-1.0..=1.0).contains(&value) => {
            return Err(invalid("入力axisの値域"));
        }
        InputValue::Pointer { x, y } if !x.is_finite() || !y.is_finite() => {
            return Err(invalid("入力pointerの有限性"));
        }
        _ => {}
    }
    if matches!(
        binding,
        InputBinding::Keyboard(_) | InputBinding::MouseButton(_)
    ) && !matches!(value, InputValue::Button(_))
    {
        return Err(invalid("物理buttonの値型"));
    }
    if matches!(binding, InputBinding::Pointer(_))
        && !matches!(
            value,
            InputValue::Pointer { .. } | InputValue::PointerUnavailable
        )
    {
        return Err(invalid("pointerの値型"));
    }
    Ok(())
}
fn invalid(context: &'static str) -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, context)
}

// 記録はメモリ配置から独立したtag付き形式です。OS時刻は保存せず、割当済みtickを保存します。
impl StateCodec for InputBinding {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        match self {
            Self::Keyboard(key) => {
                out.u8(0)?;
                key.encode(out)
            }
            Self::MouseButton(button) => {
                out.u8(1)?;
                out.u32(*button)
            }
            Self::Gamepad { player, control } => {
                out.u8(2)?;
                out.u16(*player)?;
                control.encode(out)
            }
            Self::Pointer(pointer) => {
                out.u8(3)?;
                out.u16(*pointer)
            }
            Self::Custom(key) => {
                out.u8(4)?;
                key.encode(out)
            }
        }
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(match input.u8()? {
            0 => Self::Keyboard(StableKey::decode(input)?),
            1 => Self::MouseButton(input.u32()?),
            2 => Self::Gamepad {
                player: input.u16()?,
                control: StableKey::decode(input)?,
            },
            3 => Self::Pointer(input.u16()?),
            4 => Self::Custom(StableKey::decode(input)?),
            _ => return Err(invalid("入力bindingのtag")),
        })
    }
}
impl StateCodec for InputValue {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        match self {
            Self::Button(value) => {
                out.u8(0)?;
                value.encode(out)
            }
            Self::Axis(value) => {
                out.u8(1)?;
                value.encode(out)
            }
            Self::Pointer { x, y } => {
                out.u8(2)?;
                x.encode(out)?;
                y.encode(out)
            }
            Self::PointerUnavailable => out.u8(3),
        }
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(match input.u8()? {
            0 => Self::Button(bool::decode(input)?),
            1 => Self::Axis(f32::decode(input)?),
            2 => Self::Pointer {
                x: f64::decode(input)?,
                y: f64::decode(input)?,
            },
            3 => Self::PointerUnavailable,
            _ => return Err(invalid("入力値のtag")),
        })
    }
}
impl StateCodec for InputEvent {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        validate(self.binding, self.value)?;
        out.u32(self.source)?;
        out.u64(self.sequence)?;
        self.binding.encode(out)?;
        self.value.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let event = Self {
            source: input.u32()?,
            sequence: input.u64()?,
            binding: InputBinding::decode(input)?,
            value: InputValue::decode(input)?,
        };
        validate(event.binding, event.value)?;
        Ok(event)
    }
}
impl StateCodec for TickInput {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        canonical_events(&self.events)?;
        out.u64(self.tick)?;
        self.reset.encode(out)?;
        out.u32(u32::try_from(self.events.len()).map_err(|_| invalid("入力record数"))?)?;
        for event in &self.events {
            event.encode(out)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let tick = input.u64()?;
        let reset = bool::decode(input)?;
        let count = input.u32()? as usize;
        // PointerUnavailableはsource 4 + sequence 8 + pointer binding 3 + 値tag 1の
        // 16byteです。最短recordに合わせて偽の個数による巨大確保を先に防ぎます。
        if count > input.remaining() / 16 {
            return Err(invalid("入力record数と残byte数"));
        }
        let mut events = Vec::new();
        events
            .try_reserve(count)
            .map_err(crate::foundation::error::allocation)?;
        for _ in 0..count {
            events.push(InputEvent::decode(input)?);
        }
        canonical_events(&events)?;
        Ok(Self {
            tick,
            reset,
            events,
        })
    }
}
fn canonical_events(events: &[InputEvent]) -> EngineResult<()> {
    if events
        .windows(2)
        .any(|pair| (pair[0].source, pair[0].sequence) >= (pair[1].source, pair[1].sequence))
    {
        return Err(invalid("入力記録の順序または重複"));
    }
    Ok(())
}
