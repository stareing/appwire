## 适用场景
- 演示 app-mcp：让模型像用户一样操作网页中的待办、商品、购物车与订单。

## 能力范围
- 待办（「待办」页签）：新增、勾选/取消勾选、删除；资源 `todos.list` 给出全部待办。
- 商品：`catalog.search` 按名称或分类搜索，任何页签下都可用。
- 店铺：`info` 营业信息、`deliveryEstimate` 配送时效估算，任何页签下都可用。
- 商品页：`products.filter` 筛选、`products.open` 打开详情；详情对话框打开时只有 `productDialog.*` 可用。
- 购物车页：加入商品、移除条目、结算；资源 `cart.state` 给出条目、总价与可用收货地址。
- 订单页：`orders.list` 列出已下的订单。

## 典型流程
- 添加待办：`todos.add` → 需要时 `todos.toggle`。
- 购物：`catalog.search` → `cart.add({ productId, qty })`。
- 删除最贵的商品：读取 `cart.state` → `cart.removeItem({ itemId })`。
- 结算：读取 `cart.state` 取得 `addressId` → `cart.checkout`。

## 前置条件
- 商品、购物车、订单页的工具不在当前页面时也可直接调用，App 会先切换到该页面（可用 `apps.page` 查看页面工具）；对话框打开时不切换。
- 待办工具只在「待办」页签打开时出现。
- 购物车为空时不能结算（`cart.checkout` 不可用）。

## 不支持的操作
- 退款、修改订单、修改收货地址、登录与账户管理。

## 风险说明
- `cart.checkout` 是支付操作，执行前应向用户确认商品、金额与地址。
- `todos.remove` 会永久删除待办。
