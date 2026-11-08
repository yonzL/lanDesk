import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';

import '../common.dart';
import '../models/platform_model.dart';
import '../utils/multi_window_manager.dart';

/// lanDesk 的主界面：只看本机 `addressbook.json`，不连任何服务器。
///
/// 数据全程来自 Rust 侧（`bind.landeskBookList()` 返回 JSON 数组），Dart 只负责画和触发动作，
/// 这样 Web 目标换一套实现时也不会牵动这里的逻辑。
///
/// 每 2 秒重读一次文件：CLI（`--landesk-book-add`、`--landesk-import`）改完直接反映出来，
/// 不需要重启界面。
class LandeskAddressBookPage extends StatefulWidget {
  const LandeskAddressBookPage({Key? key}) : super(key: key);

  @override
  State<LandeskAddressBookPage> createState() => _LandeskAddressBookPageState();
}

class _Entry {
  _Entry({
    required this.group,
    required this.subgroup,
    required this.name,
    required this.addr,
    required this.key,
    required this.pin,
    required this.fingerprint,
  });

  final String group;
  final String subgroup;
  final String name;
  final String addr;
  final String? key;

  /// `never` / `unverified` / `verified`
  final String pin;
  final String? fingerprint;

  String get label => name.isNotEmpty ? name : addr;

  Color get statusColor => switch (pin) {
        'verified' => const Color(0xFF2E7D32),
        'unverified' => const Color(0xFFF9A825),
        _ => const Color(0xFF9E9E9E),
      };

  String get statusTip => switch (pin) {
        'verified' => '指纹已核对（已验证）',
        'unverified' => '已记录指纹，但还没核对（双击连一次后到指纹页核对）',
        _ => '从未连接过',
      };
}

class _LandeskAddressBookPageState extends State<LandeskAddressBookPage> {
  Timer? _timer;
  List<_Entry> _entries = const [];
  String? _error;

  @override
  void initState() {
    super.initState();
    _reload();
    _timer = Timer.periodic(const Duration(seconds: 2), (_) => _reload());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _reload() async {
    final raw = await bind.landeskBookList();
    if (raw.startsWith('error:')) {
      if (mounted) setState(() => _error = raw.substring(6));
      return;
    }
    try {
      final list = (jsonDecode(raw) as List).cast<Map<String, dynamic>>();
      final entries = list
          .map((item) => _Entry(
                group: (item['group'] as String?) ?? '',
                subgroup: (item['subgroup'] as String?) ?? '',
                name: (item['name'] as String?) ?? '',
                addr: (item['addr'] as String?) ?? '',
                key: item['key'] as String?,
                pin: (item['pin'] as String?) ?? 'never',
                fingerprint: item['fingerprint'] as String?,
              ))
          .toList();
      if (mounted) {
        setState(() {
          _entries = entries;
          _error = null;
        });
      }
    } catch (err) {
      if (mounted) setState(() => _error = '地址簿解析失败：$err');
    }
  }

  Future<void> _open(_Entry entry) async {
    // 走上游原本的多窗口链路：新窗口是同一个进程里的第二个 Flutter 引擎，窗口里的 RemotePage
    // 直接 `start()`；身份由 Rust 侧的 `client::_start` 从地址簿解析，不需要提前登记。
    try {
      await rustDeskWinManager.newRemoteDesktop(entry.addr);
    } catch (err) {
      _toast('无法打开会话窗口：$err');
    }
  }

  void _toast(String text) {
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(text)));
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        _toolbar(),
        const Divider(height: 1),
        Expanded(child: _body()),
      ],
    );
  }

  Widget _toolbar() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
      child: Row(
        children: [
          Text('地址簿', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(width: 8),
          Text('${_entries.length} 台',
              style: Theme.of(context).textTheme.bodySmall),
          const Spacer(),
          TextButton.icon(
            onPressed: _showAddDialog,
            icon: const Icon(Icons.add, size: 18),
            label: const Text('添加'),
          ),
          IconButton(
            onPressed: _reload,
            icon: const Icon(Icons.refresh, size: 18),
            tooltip: '重新读取 addressbook.json',
          ),
        ],
      ),
    );
  }

  Widget _body() {
    if (_error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Text(_error!, textAlign: TextAlign.center),
        ),
      );
    }
    if (_entries.isEmpty) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Text('地址簿是空的'),
              const SizedBox(height: 8),
              Text(
                '手工添加，或在命令行导入 CSV：\n'
                'landesk-client --landesk-import book.csv',
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ],
          ),
        ),
      );
    }
    final groups = <String, Map<String, List<_Entry>>>{};
    for (final entry in _entries) {
      final group = entry.group.isEmpty ? '默认分组' : entry.group;
      final subgroup = entry.subgroup.isEmpty ? '' : entry.subgroup;
      groups.putIfAbsent(group, () => {}).putIfAbsent(subgroup, () => []).add(entry);
    }
    final keys = groups.keys.toList()..sort();
    return ListView(
      children: [
        for (final group in keys) _groupTile(group, groups[group]!),
      ],
    );
  }

  Widget _groupTile(String group, Map<String, List<_Entry>> subgroups) {
    final subKeys = subgroups.keys.toList()..sort((a, b) {
        if (a.isEmpty) return -1;
        if (b.isEmpty) return 1;
        return a.compareTo(b);
      });
    final total = subgroups.values.fold<int>(0, (sum, list) => sum + list.length);
    return ExpansionTile(
      initiallyExpanded: true,
      title: Text('$group（$total）'),
      children: [
        for (final sub in subKeys)
          if (sub.isEmpty)
            for (final entry in subgroups[sub]!) _entryRow(entry)
          else
            ExpansionTile(
              initiallyExpanded: true,
              tilePadding: const EdgeInsets.only(left: 32, right: 16),
              title: Text('$sub（${subgroups[sub]!.length}）'),
              children: [
                for (final entry in subgroups[sub]!) _entryRow(entry, indent: 24),
              ],
            ),
      ],
    );
  }

  Widget _entryRow(_Entry entry, {double indent = 0}) {
    return InkWell(
      onDoubleTap: () => _open(entry),
      child: Padding(
        padding: EdgeInsets.only(left: 16 + indent, right: 8, top: 6, bottom: 6),
        child: Row(
          children: [
            Tooltip(
              message: entry.statusTip,
              child: Container(
                width: 9,
                height: 9,
                decoration:
                    BoxDecoration(color: entry.statusColor, shape: BoxShape.circle),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(entry.label, overflow: TextOverflow.ellipsis),
                  Text(
                    entry.addr,
                    style: Theme.of(context).textTheme.bodySmall,
                    overflow: TextOverflow.ellipsis,
                  ),
                ],
              ),
            ),
            if (entry.key != null)
              Tooltip(
                message: '身份：${entry.key}',
                child: const Icon(Icons.vpn_key, size: 14),
              ),
            PopupMenuButton<String>(
              tooltip: '',
              icon: const Icon(Icons.more_horiz, size: 18),
              onSelected: (value) => _onAction(value, entry),
              itemBuilder: (context) => [
                const PopupMenuItem(value: 'open', child: Text('连接')),
                PopupMenuItem(
                  value: 'verify',
                  enabled: entry.pin == 'unverified',
                  child: const Text('标记指纹为已验证'),
                ),
                if (entry.fingerprint != null)
                  PopupMenuItem(
                    value: 'fingerprint',
                    child: Text('指纹 ${entry.fingerprint}'),
                  ),
                const PopupMenuItem(value: 'remove', child: Text('删除')),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _onAction(String action, _Entry entry) async {
    switch (action) {
      case "open":
        await _open(entry);
      case "verify":
        final err = await bind.landeskBookVerify(addr: entry.addr);
        if (err.isNotEmpty) _toast(err.replaceFirst('error:', ''));
        _reload();
      case 'fingerprint':
        _toast('${entry.label}：${entry.fingerprint}');
      case "remove":
        final err = await bind.landeskBookRemove(addr: entry.addr);
        if (err.isNotEmpty) _toast(err.replaceFirst('error:', ''));
        _reload();
    }
  }

  Future<void> _showAddDialog() async {
    final groupCtl = TextEditingController();
    final subgroupCtl = TextEditingController();
    final nameCtl = TextEditingController();
    final addrCtl = TextEditingController();
    final keyCtl = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('添加机器'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: groupCtl,
              decoration: const InputDecoration(labelText: '组'),
            ),
            TextField(
              controller: subgroupCtl,
              decoration: const InputDecoration(labelText: '子组'),
            ),
            TextField(
              controller: nameCtl,
              decoration: const InputDecoration(labelText: '显示名'),
            ),
            TextField(
              controller: addrCtl,
              decoration:
                  const InputDecoration(labelText: '地址:端口', hintText: '10.0.0.5:21118'),
            ),
            TextField(
              controller: keyCtl,
              decoration: const InputDecoration(
                  labelText: '身份（留空用默认私钥）', hintText: 'master-xxx'),
            ),
          ],
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('取消')),
          FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('保存')),
        ],
      ),
    );
    if (ok != true) return;
    final err = await bind.landeskBookUpsert(
      group: groupCtl.text,
      subgroup: subgroupCtl.text,
      name: nameCtl.text,
      addr: addrCtl.text.trim(),
      key: keyCtl.text.trim(),
    );
    if (err.isNotEmpty) {
      _toast(err.replaceFirst('error:', ''));
    } else {
      _reload();
    }
  }
}
