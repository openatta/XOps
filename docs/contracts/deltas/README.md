# 进行中的变更各自的 delta

一个变更一个目录，目录名就是变更名（用分支名最省事）：

```text
deltas/<变更名>/api.md      改了对外接口就写
deltas/<变更名>/rust.md     改了 crate 接缝就写
deltas/<变更名>/data.md     改了数据库就写
```

**没碰某个面就不建那个文件**，不需要写一份全是 `(none)` 的空 delta。
但**建了就要五节齐全**——`(none)` 是"这一节我看过，确实没有"，不是省略。

改完之后跑 `node scripts/contracts.mjs sync`，它会把 delta 合进对应基线并**删除本目录**，
然后连同实现一起提交。**别手工改基线**——合并有 ADD 已存在、MODIFY 不存在这类判定，
手改绕过它们，基线就不再是"被批准过的记录"了。见 [README §5](../README.md#5-忘了跑-sync-会怎样)。

模板见 [`TEMPLATE.md`](TEMPLATE.md)。
