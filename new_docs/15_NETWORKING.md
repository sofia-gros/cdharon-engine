# 協力プレイとサーバー

## 基本方式

networkは任意のプラグインとして提供し、v1ではサーバー権威の協力プレイを実装する。専用サーバーも、local hostとclientを一つのアプリ内で動かす構成も、同じWorldと操作APIを使う。

サーバーが地形、素材、物理、inventory、damage、装置、進行を確定する。clientは入力と操作の意図を送る。clientの位置やitem数をそのまま正として採用しない。複数OSの浮動小数点の完全一致を通信の成立条件にしない。

上記は標準OwnershipPolicyである。[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)に従い、作者は所有権・検証・複製・transportを置き換え、対象作品の権限分担を実装できる。client権限を認める場合も、その範囲とvalidatorを明示し、未検証の値を自動採用しない。独自channel/provider stateとイベントも複製対象にできる。

transportはQuinnによるQUICをadapterで利用する方針とする。接続・暗号・輻輳制御の実装と、ゲームの複製schemaを分ける。インターネット接続の認証・証明書設定、LAN用の明示的な開発設定を提供し、接続相手の識別を無条件で省略しない。

## 接続の順序

1. protocol、engine profile、game build、pluginとcontent manifestを交換する。
2. 権限、player identity、必要な資産packの合致を確認する。不一致なら開始前に理由を返す。
3. 関心regionの基準Snapshotとtickを送り、受信したbaselineを確認する。
4. 以後の入力、command、revision delta、Entity状態、確定eventを送る。
5. 再接続時はsession tokenとack世代から差分を再開し、基準が失われていればSnapshotを再送する。

途中参加中の不完全なchunkを物理の正として公開しない。個体参照はNetIdとWorld内IDの対応で解決し、接続ごとにruntime allocatorの偶然のIDへ依存させない。

## 伝送データ

| 種類 | 方式と内容 |
| --- | --- |
| 定義・chunk・inventory・取引receipt | reliable stream、revisionと長さ、checksum、適用tick |
| 入力 | sequence、対象tick、player、actionの値。重複と不正な未来時刻を拒否 |
| 移動Snapshot | datagramまたは最新値のstream。tickとbaseline IDを付ける |
| 確定event | EventIdで重複排除し、必要なものはack付きで送る |
| 作者component | 登録したreplication schemaとcodecで送る |

chunk deltaはrevisionを前提にし、欠落した差分の上へ続きだけ適用しない。baseline不一致は再同期する。圧縮前後の上限、接続ごとのbyte予算とcommand予算を検証する。

## 関心領域と画面外

clientのcamera、player、遠隔UIの必要領域からinterest setを作る。サーバーのSimulate leaseとは別に管理し、送信しない区域でも必要な装置とWorld物理は進められる。

権限に応じて見える情報を制限し、未探索mapや秘密のcomponentを無条件で全clientへ送らない。interest変更は安定したrevision順にchunkとEntityを出入りさせる。

## 予測と補正

clientの操作応答はlocal controllerの予測を提供し、サーバー結果で補正する。地形やbodyの動作に依存する場合は必要な近傍Snapshotを使い、未知領域を空気と仮定しない。表示補間と予測履歴を正規saveへ混ぜない。

damage、item取引、地形編集はreceiptを待って確定する。予測表示は未確定と区別し、拒否された場合に表示を戻す。完全な世界全体のrollbackを、単純な移動補正の前提にはしない。

## 作者APIと検証

作者は権限付きcommand、replicated component、remote eventを登録する。decode後にschema、ID、range、所有権、到達距離、sequenceを検証する。未検証payloadから任意のRust関数を呼び出さない。

4clientの参加・離脱、100ms往復遅延、2%packet loss、順序入れ替え、再接続、chunk再同期、同時採掘とcraft、遠隔装置、保存中の切断を検証する。サーバーと全clientの正規revisionが最終的に一致し、itemの二重取得がないことを確認する。

4clientは最小の基盤試験であり参加人数の上限ではない。完全再現の認定は対象buildの参加人数・mode・権限に合わせた追加profileで行う。人数や原作の通信挙動を4人協力へ縮めてV1-32を合格にしない。
