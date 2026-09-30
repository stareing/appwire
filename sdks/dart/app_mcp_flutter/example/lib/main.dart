// app-mcp Flutter 示例：一个购物车，把"浏览商品、加入 / 移出购物车、结算"暴露为 MCP 工具。
//
// 运行前需要原生库 libapp_mcp（bindings/c）。桌面端可用环境变量 APP_MCP_NATIVE_PATH 指定路径，
// 移动端需把库打包进 App（Android jniLibs、iOS 静态链接）。
import 'package:app_mcp_flutter/app_mcp_flutter.dart';
import 'package:flutter/material.dart';

class Product {
  const Product(this.id, this.name, this.price);
  final String id;
  final String name;
  final int price; // 分

  Map<String, Object?> toJson() => {'id': id, 'name': name, 'price': price};
}

const products = [
  Product('apple', '苹果', 350),
  Product('milk', '牛奶', 1200),
  Product('bread', '面包', 800),
];

void main() {
  final client = AppMcp(
    appId: 'flutter-shop',
    appName: 'Flutter 商店',
    appVersion: '0.1.0',
    overview: const AppOverview(
      summary: '演示用购物车：浏览商品、加入或移出购物车、结算',
      body: '## 典型流程\n1. 读取资源 `cart` 或调用 `products.list`\n2. `cart.add` 加入商品\n3. `cart.checkout` 结算（需要用户确认）',
      locale: 'zh-CN',
    ),
  )..start();
  runApp(AppMcpScope(client: client, disposeClient: true, child: const ShopApp()));
}

class ShopApp extends StatelessWidget {
  const ShopApp({super.key});

  @override
  Widget build(BuildContext context) => MaterialApp(
        title: 'Flutter 商店',
        theme: ThemeData(colorSchemeSeed: Colors.teal, useMaterial3: true),
        home: const CartPage(),
      );
}

class CartPage extends StatefulWidget {
  const CartPage({super.key});

  @override
  State<CartPage> createState() => _CartPageState();
}

class _CartPageState extends State<CartPage> with McpToolsMixin {
  /// 商品 ID → 数量。每次修改都换一个新 Map，便于 McpResource 用 changeToken 感知变化。
  Map<String, int> _cart = const {};
  String? _lastOrder;

  int get _total =>
      _cart.entries.fold(0, (sum, e) => sum + products.firstWhere((p) => p.id == e.key).price * e.value);

  Map<String, Object?> _cartJson() => {
        'items': [
          for (final e in _cart.entries) {'id': e.key, 'qty': e.value}
        ],
        'total': _total,
      };

  Product _product(Object? id) => products.firstWhere((p) => p.id == id,
      orElse: () => throw ToolCallError(ErrorKind.invalidInput, '没有这个商品：$id'));

  void _add(String id, int qty) => setState(() => _cart = {..._cart, id: (_cart[id] ?? 0) + qty});

  void _remove(String id) => setState(() => _cart = {..._cart}..remove(id));

  @override
  Widget build(BuildContext context) {
    // hook 风格：每次 build 刷新 handler，不重新注册。
    useMcpTool('products.list',
        description: '列出可购买的商品（价格单位：分）',
        risk: Risk.read,
        handler: (args, ctx) => [for (final p in products) p.toJson()]);

    return Scaffold(
      appBar: AppBar(title: const Text('Flutter 商店')),
      body: McpToolGroup(
        name: 'cart-page',
        child: McpResource(
          name: 'cart',
          description: '当前购物车内容与总价',
          read: _cartJson,
          changeToken: _cart,
          child: McpTool(
            name: 'cart.add',
            description: '把商品加入购物车',
            inputSchema: const {
              'type': 'object',
              'properties': {
                'id': {'type': 'string', 'description': '商品 ID'},
                'qty': {'type': 'integer', 'minimum': 1, 'default': 1},
              },
              'required': ['id'],
            },
            handler: (args, ctx) {
              final p = _product(args['id']);
              _add(p.id, (args['qty'] as int?) ?? 1);
              return ToolResult(_cartJson(), stateHints: const ['cart']);
            },
            child: McpTool(
              name: 'cart.remove',
              description: '把商品移出购物车',
              inputSchema: const {
                'type': 'object',
                'properties': {
                  'id': {'type': 'string'}
                },
                'required': ['id'],
              },
              enabled: _cart.isNotEmpty,
              handler: (args, ctx) {
                _remove(_product(args['id']).id);
                return ToolResult(_cartJson(), stateHints: const ['cart']);
              },
              child: McpTool(
                name: 'cart.checkout',
                description: '结算购物车',
                risk: Risk.payment,
                activation: Activation.foreground,
                enabled: _cart.isNotEmpty,
                handler: _checkout,
                child: _buildBody(),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Future<Object?> _checkout(Map<String, dynamic> args, ToolContext ctx) async {
    if (_cart.isEmpty) throw ToolCallError(ErrorKind.toolDisabled, '购物车为空');
    // 结算需要用户在界面上确认；调用被取消（超时、断线）时关闭对话框。
    final confirmed = await Future.any([
      showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('确认结算'),
          content: Text('共 ${(_total / 100).toStringAsFixed(2)} 元'),
          actions: [
            TextButton(onPressed: () => Navigator.pop(context, false), child: const Text('取消')),
            FilledButton(onPressed: () => Navigator.pop(context, true), child: const Text('结算')),
          ],
        ),
      ),
      ctx.cancelled.then((_) {
        if (mounted) Navigator.of(context).maybePop();
        return false;
      }),
    ]);
    if (confirmed != true) throw ToolCallError(ErrorKind.userRejected, '用户取消了结算');
    final order = 'order-${DateTime.now().millisecondsSinceEpoch}';
    final total = _total;
    setState(() {
      _cart = const {};
      _lastOrder = order;
    });
    return ToolResult({'orderId': order, 'total': total}, stateHints: const ['cart']);
  }

  Widget _buildBody() => ListView(
        children: [
          for (final p in products)
            ListTile(
              title: Text(p.name),
              subtitle: Text('${(p.price / 100).toStringAsFixed(2)} 元'),
              trailing: Row(mainAxisSize: MainAxisSize.min, children: [
                if (_cart[p.id] != null) Text('× ${_cart[p.id]}'),
                IconButton(icon: const Icon(Icons.add_shopping_cart), onPressed: () => _add(p.id, 1)),
                if (_cart[p.id] != null)
                  IconButton(icon: const Icon(Icons.remove_circle_outline), onPressed: () => _remove(p.id)),
              ]),
            ),
          const Divider(),
          ListTile(
            title: const Text('合计'),
            trailing: Text('${(_total / 100).toStringAsFixed(2)} 元'),
          ),
          if (_lastOrder != null) ListTile(title: Text('上一笔订单：$_lastOrder')),
          StreamBuilder<McpConnectionState>(
            stream: AppMcpScope.of(context).states,
            builder: (context, snap) => ListTile(
              leading: const Icon(Icons.hub_outlined),
              title: Text('app-mcp：${snap.data?.status.name ?? '未知'}'),
            ),
          ),
        ],
      );
}
