# App Store 内购（StoreKit）与沙盒验证清单

面向：笺读 `com.jiandu.app`　·　买断型内购 `cn.jiandu.unlock.pro`
代码：`crates/app/src/storekit.rs`（已实现，见 §7）　·　排查日期 2026-11-01

---

## 0. 先看清一件事：本机现在跑不通内购

```
$ security find-identity -v -p codesigning
     0 valid identities found
```

**没有任何签名身份。** 而 StoreKit 的沙盒环境认的是「已签名 + 已在 App Store
Connect 注册」的包：

| 你想要 | 前提 | 现状 |
|---|---|---|
| 真机试买（沙盒价、假 Apple ID） | ① 付费开发者账号 ② App Store Connect 有 App 记录 ③ 建好 IAP ④ 用 **TestFlight** 装 | ❌ 四个都没有 |
| 本地预演内购（不联网、不花钱） | 需要 Xcode 工程 + `.storekit` 配置文件（`SKTestSession`） | ❌ 本项目是 cargo 构建，无 Xcode 工程 |
| 验证沙盒文件访问 | 只要 `SANDBOX=1` + ad-hoc 签名 | ✅ **今天就能做** |

所以内购这条链的时间顺序是固定的，绕不过去：

```
报名开发者计划($99/年) → 建 App 记录 → 建 IAP → 签名上传 → TestFlight 内测 → 才能真买
```

在此之前能做的，只有「把代码写对 + 用假凭据把 UI 走通」。代码已经写完了。

---

## 1. 开发者账号（Day 0，1–3 天，唯一要花钱的一步）

1. <https://developer.apple.com/programs/> 用 Apple ID 报名，个人身份即可
   （个人账号在 App Store 上显示的是**个人姓名**，公司名需要 D-U-N-S 编号；
   如果要以「笺读」名义卖，办个体户/公司再走组织账号）。
2. 通过后拿到 **Team ID**（10 位），记下来——后面签名与后台都要用。
3. Xcode → Settings → Accounts 登录同一 Apple ID → Manage Certificates →
   建 **Apple Distribution** 与 **Mac Installer Distribution**（各一张）。
4. 复查：`security find-identity -v -p codesigning` 应显示 ≥1 条。

> 这一步不做，后面 2–6 全部无法进行。所以**先报名，再谈别的**。

---

## 2. App Store Connect：建 App 记录（Day 1）

| 字段 | 值 | 备注 |
|---|---|---|
| 平台 | macOS | |
| 名称 | 笺读 | 30 字以内，不能与已有重名 |
| 主要语言 | 简体中文 | |
| Bundle ID | `com.jiandu.app` | **必须与 `make-mac-app.sh` 里 `BUNDLE_ID` 逐字一致** |
| SKU | `jiandu` | 内部编号，随便但别改 |
| 用户访问权限 | 完全访问 | |

---

## 3. 建内购（Day 1，最容易出错的一步）

App Store Connect → 你的 App → **App 内购买项目** → 「+」

| 字段 | 值 |
|---|---|
| 类型 | **非消耗型**（买断，可恢复） |
| 参考名称 | 解锁全部功能 |
| **产品 ID** | **`cn.jiandu.unlock.pro`** |
| 价格 | ¥198（首发两周 ¥148 用「促销价格」另设，不要改基础价） |
| 显示名称（zh-Hans） | 解锁全部功能 |
| 描述（zh-Hans） | 解锁导出与印制：PNG 出图、PDF 印制、Markdown、朱批录、Anki / Obsidian 卡片导出。阅读、批注、卡片匣不受限。 |
| 审核截图 | 解锁页截图（等 §7 的 UI 做完再传） |

⚠️ **产品 ID 必须与 `storekit.rs::UNLOCK_PRODUCT_ID` 逐字相同**。写错一个字符，
表现是「已付费用户被判成未购买」——最花钱也最难看的一类工单。

首次建 IAP 必须与**至少一个** App 版本一起提交审核（IAP 不能单独提审）。
建议把 IAP 挂在 v1.9.23 这个版本下一起提。

---

## 4. 签名与 entitlements

### 4.1 必须的 entitlement

商店版必须有沙盒（App Store 强制），`jiandu.entitlements`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.app-sandbox</key><true/>
    <!-- 问小笺：HTTPS 调 OpenAI 兼容接口 -->
    <key>com.apple.security.network.client</key><true/>
    <!-- 导入原图 / 书籍，导出 PNG/PDF/MD/CSV -->
    <key>com.apple.security.files.user-selected.read-write</key><true/>
</dict>
</plist>
```

* **StoreKit 不需要额外 entitlement**，只要 app-sandbox 开着即可。
* 不申请 `files.downloads` / `files.pictures`（导出走用户选的保存面板，属
  `user-selected`，够了）。

### 4.2 签名方式

`make-mac-app.sh` 现在是 ad-hoc（`codesign -s -`），**ad-hoc 包不能上传、不能试买**。
上架时改成：

```bash
codesign --force --deep --options runtime \
  --entitlements jiandu.entitlements \
  --sign "Apple Distribution: <你的名字> (<TEAMID>)" \
  "dist/笺读.app"
```

上传用 `xcrun notarytool submit`（Developer ID 分发）或
`xcrun altool --upload-app` / Transporter（App Store 分发）。

> 建议：保留 `make-mac-app.sh` 的 ad-hoc 路径（官网版），
> 另加 `STORE=1` 分支走 App Store 签名 —— 两条分发渠道不互相污染。

---

## 5. 沙盒真机验证（不用等账号，今天就能做）

```bash
SANDBOX=1 ./scripts/make-mac-app.sh      # ad-hoc 签名 + 沙盒 entitlements
open dist/笺读.app
```

逐条走：

| # | 动作 | 期望 | 常见违规 |
|---|---|---|---|
| 1 | 首启 | 正常进书架，无白屏 | 数据目录写死在仓库路径 → 沙盒下无权限 |
| 2 | 导入 TXT / EPUB / MD | 成功入库 | 用 `std::fs` 读用户拖进来的路径但没走 `NSOpenPanel` → 无权限 |
| 3 | 导入扫描版 PDF | 底图显示 + 「需 OCR」提示 | 无 |
| 4 | 导出 PNG | 保存面板 → 落到用户选的位置 | 直接写 `~/Desktop` → 拒绝 |
| 5 | 印制 PDF | 同上 | 写到 `/tmp` 后 `save_bytes` 搬家（应可用） |
| 6 | 自备字体 | 把 ttf 放进 `~/Library/Containers/com.jiandu.app/Data/fonts/` 后重启可见 | 去扫 `~/Library/Fonts/` → 无权限 |
| 7 | 问小笺 | 能发出请求（`network.client`） | 忘记 entitlement → 直接失败 |
| 8 | 卡片匣 / 批注 | 落盘在 Container 内，重启仍在 | 写到 `~/Documents` → 拒绝 |
| 9 | 库文件位置 | `~/Library/Containers/com.jiandu.app/Data/Library/Application Support/...` | — |
| 10 | ⌘Q 退出再开 | 状态保持 | — |

> 沙盒下**数据目录会整体搬到 Container 里**。老用户从官网版迁到商店版时，
> 书库看起来「空了」——需要在首次启动时做一次迁移（从旧路径复制），
> 否则会被当成丢数据的差评。这条要在提审前处理。

---

## 6. 内购验证（拿到账号后，用 TestFlight）

1. App Store Connect → 用户与访问 → **沙盒测试员** → 新建一个**不存在的**
   Apple ID（如 `jiandu.test01@example.com`，不需要真邮箱）。
2. 上传构建（`xcrun altool` / Transporter）→ TestFlight → 加自己为内部测试员。
3. 在 TestFlight 装包 → 点「解锁全部功能」→ 用**沙盒测试员**账号登录。
4. 逐条验：

| # | 场景 | 期望 |
|---|---|---|
| 1 | 新装，未购买 | 导出被拦，提示指向解锁页（不是「激活码」） |
| 2 | 解锁页 | **显示真实价格**（`¥198.00`，来自 StoreKit 而非写死） |
| 3 | 点购买 → 取消 | **不报错**，回到可再买状态（错误码 2 已被吞掉） |
| 4 | 点购买 → 成功 | 立刻可导出，无需重启 |
| 5 | 购买中杀进程 → 重开 | 仍在，且不重复弹支付（`finishTransaction` 已调） |
| 6 | 已购买 → 删 App 重装 | 显示未购买 → 点「恢复购买」→ 立刻解锁 |
| 7 | 断网 → 点购买 | 出人话错误（不是崩溃、不是转圈） |
| 8 | 换一台 Mac，同账号 | 恢复购买可用 |
| 9 | 退款 | 下次启动仍解锁（本地缓存不会自动撤销）→ **这是已知债**，见 §8 |

日志：`JIANDU_STORE_DEBUG=1 open dist/笺读.app` 会打 `[storekit] ...`
（交易状态、商品数量、恢复结果）。

---

## 7. 代码现状（已完成）

`crates/app/src/storekit.rs` 三层：

```
公开 API     prepare / purchased / phase / price / purchase / restore / last_error
纯状态机     Phase 8 态 + 原子量 + 单元测试（不碰 Apple，cargo test 可跑）
后端         StoreKit 1（SKPaymentQueue + SKProductsRequest，objc 直调）
```

已实现的行为：

* 启动即挂 `SKPaymentQueue` 观察者（会重投上次未 finish 的交易）；
* 用 `SKProductsRequest` 取**真实本地化价格**，并在商品查不到时把排查方向
  直接写上屏（后台没配 / Bundle ID 不符 / 未签名）；
* 购买成功 / 恢复成功 → 写 `NSUserDefaults`（键 `cn.jiandu.purchased`）+ 解锁 + `finishTransaction`；
* 用户取消（`SKErrorPaymentCancelled` = 2）→ **不报错**，回到可再买状态；
* 待批准（Deferred）→ 保持「进行中」并给话；
* 恢复跑完却一笔没有 → 明确告知「此账号无购买记录」，不留在转圈；
* 非商店版（默认构建）：整套走 stub，`phase=NoProduct`，UI 不会误显内购。

开关：

| 变量 / feature | 作用 |
|---|---|
| `--features appstore` | 启用真 StoreKit 后端；同时摘掉激活屏 / 内测码（2.4.5(vi)） |
| `JIANDU_STORE_DEBUG=1` | 打 storekit 日志 |
| `JIANDU_STORE_UNLOCK=1` | **仅 debug 构建**假解锁，用于把 UI 走通；release 里该分支恒 false |

---

## 8. 已知债（写在这里，免得忘）

1. **退款不会自动撤销**：本地缓存是布尔，退款后不会自动回收权限。
   正解：启动时读 `appStoreReceiptURL` 那张收据、本地校验其中是否有该 IAP
   （PKCS#7，需要 ASN.1 解析）。买断价的产品可以先接受，但要知道这条。
2. **`defaults write com.jiandu.app cn.jiandu.purchased -bool true` 能白拿**。
   同上，收据校验才是正解；反破解投入与 198 元的客单价不成比例。
3. **官网版 → 商店版的数据迁移**：沙盒把数据目录搬进 Container，老用户会
   觉得书库空了，必须在首启时迁移（见 §5 末尾）。
4. **StoreKit 1 已 deprecated**（macOS 15 起）。仍可用、仍能过审；将来迁
   StoreKit 2 只需替换 `storekit.rs` 的 `backend`，上层不动。

---

## 9. 提审备注（给审核员看的话）

> 本应用为古籍阅读器。**阅读、批注、卡片功能完全免费**；
> 「解锁全部功能」（非消耗型）仅解锁**导出与印制**（PNG / PDF / Markdown /
> Anki / Obsidian）。解锁页在「更多 → 工具 → 解锁全部功能」，
> 购买页显示的是 StoreKit 返回的真实价格；同一页面提供「恢复购买」。
> 测试账号：（提审时填沙盒测试员账号）
