---
id: SPEC-WORKTREE
title: 'worktreeとファイル操作'
status: draft
documentVersion: '0.2'
updated: '2026-11-05'
---

# worktreeとファイル操作

[文書目次](../README.md) · [更新ルール](../maintenance.md)

> statusは`draft`です。実装済みとして扱う範囲は、コードと試験結果で確認します。

この文書の管理対象：作成・採用、コピー元、コピー条件、Git hookとの関係。

<a id="source-12"></a>

## worktreeとファイル操作

### 作成

Herdrを利用する通常経路では、HerdrAdapterからworktree作成を依頼し、応答に含まれるworktreeとworkspaceの対応を保存する。別途workspaceを追加作成しない。

既存ブランチの利用と新規ブランチ作成を区別する。`base`は新規ブランチを作る場合に使い、既存ブランチを指定した際にその内容を`base`へ巻き戻さない。

別worktreeでcheckout済みのブランチに対して強制実行しない。既存checkoutの採用は`attach`で明示する。`up --herdr`は`worktree.create`を送る前に`git worktree list --porcelain`で照合し、対象ブランチが既にcheckoutされていれば作成要求を送らずに拒否する(`ifAlreadyCheckedOut: error`)。main worktreeを自動イベントの初期化対象に含めない。

<a id="attach"></a>

### 既存checkoutの採用

`worktree.mode: attach`の対象は、`attach --checkout PATH`で指定したパス、またはHerdrイベントが通知したworktreeとする。設定ファイルには対象パスを書かない。対象がGitの作業ツリーで、同じリポジトリに属することをGit CLIで確認できない場合は開始しない。

`worktree.branch`を指定した場合は、対象checkoutの現在のブランチと一致することを確認する。一致しない場合とdetached HEADの場合は開始しない。省略した場合は対象checkoutのブランチを`${inputs.branch}`等の解決に使わない。

採用したcheckoutは借用リソースとして記録し、`remove`や後片付けで削除しない。ブランチの作成・切り替え・`base`への巻き戻しは行わない。`files`のコピーは新規作成時と同じ規則で行う。

Gitの情報はCLIから取得し、`.git`ファイルや管理ディレクトリを独自に編集しない。作業ツリー一覧、共通Gitディレクトリ、対象ブランチを照合する。[S9](../reference/sources.md#s9)

### ファイルコピー

`files.source: primary`は、Gitが認識するmain worktreeを指す。ブランチ名が`main`であるディレクトリを探すという意味ではない。bareリポジトリ等でコピー元を確定できない場合は、コピーを開始せず理由を返す。

コピーするパスは許可リスト方式とする。初期版の既定では、Git未追跡かつignore対象のファイルに限る。`.env`を含め、指定されていないファイルはコピーしない。`node_modules`等を自動で追加しない。

絶対パス、`..`による脱出、親子で重複する指定、コピー先の追跡済みファイルを拒否する。途中の親ディレクトリと対象ファイルのsymlinkも既定で拒否する。検査後の差し替えに備え、実際の操作時にもパスと所有対象を再確認する。

コピー先の重複判定では、相対パスの`.`要素、連続する区切り、末尾の区切りを字句的に正規化し、パス要素単位で同一・親子関係を検出する。`.env`と`./.env`、`config/`と`config/local.env`、`config/local.env`と`config//local.env`はいずれも重複として拒否する。

対象が既に存在する場合の既定動作はエラーとする。`skip`または`replace`は明示指定した場合だけ許可する。`replace`時は一時領域に準備してから置き換え、操作記録を残す。

一連の複数ファイル操作を、OS全体として不可分なトランザクションだとは扱わない。途中で失敗した場合は、本プラグインが記録した変更だけを逆順に復旧する。実行中に利用者が変更したファイルは無条件で巻き戻さない。

### Git hookとの関係

本プラグインは`core.hooksPath`や既存hookを変更しない。既存hookが同じ初期化を行う場合は、二重実行になり得ることを検査結果へ表示する。

Gitの`post-checkout`はworktree作成以外でも実行される。`.git`がファイルかどうかだけでは「作成時に一度だけ」を判定できない。[S10](../reference/sources.md#s10) 本プラグインでは、イベント受付とRunの記録で初回適用を判定する。

Git側で有効なhookやフィルターもコマンド実行につながるため、worktree作成前の承認でその存在を確認対象に含める。`trust`は、`git rev-parse --git-path hooks`が指すディレクトリの実行可能なhook(`.sample`を除く)と、`git config`のclean・smudge・processフィルター(利用者全体の設定を含む)を承認内容に表示する。Gitの通常動作を秘密裏に無効化しない。

## 関連文書

[後片付け](cleanup.md) / [実行承認](security.md) / [Herdr連携](../integrations/herdr.md)
