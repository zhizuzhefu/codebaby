# 环境与固定环境

> `codebaby guide env` · 桌面端：侧栏「变量与环境」→ 环境管理页（切换 / 编辑变量 / 设为默认 / 新建 / 重命名 / 复制 / 删除）

## 环境

一个环境 = `envs/<名称>.json` 里的一组变量，至少有 `baseUrl`。`-e <环境>` 选择，不写用 `codebaby.json` 的 `defaultEnv`。

```bash
codebaby env list                      # 全部环境：默认 / 变量数 / 本机覆盖 / 固定在它上面的流程
codebaby env new staging --from dev    # 复制变量（本机覆盖不复制）
codebaby env rename staging test       # 同步本机覆盖、defaultEnv、固定在它上面的流程
codebaby env cp test qa
codebaby env default test
codebaby env rm qa                     # 默认环境 / 被流程固定的环境不能删
codebaby vars set baseUrl https://api.test.example -e test
```

用例里 URL 一律写 `{{baseUrl}}/路径`。写死了某个环境的域名时，validate 提示 `hardcoded-base-url`，`codebaby fmt --fix` 自动改成 `{{baseUrl}}`。

## 固定环境

本地 fixture、会写数据的造数链路只能在某个环境跑：在流程上声明 `env`。

```json project
{
  "version": 1,
  "defaultEnv": "test",
  "workflows": {
    "local-fixture": {
      "name": "本地隔离样本回归",
      "env": "local",
      "steps": [{ "run": "fixtures/collections" }]
    }
  }
}
```

```bash
codebaby workflow env local-fixture local
```

- `codebaby verify -e test`：跳过这个流程（报告里该阶段为「未执行」），其余照常
- `codebaby verify -e local` 或 `codebaby workflow local-fixture`：运行它
- `codebaby workflow local-fixture -e test`：拒绝执行并说明原因

不要在 collection.json 的 setup 里写 `if (env.baseUrl !== '…') throw new Error(…)` 来兜底：那样在其他环境跑全部用例时会报错。
validate 会提示 `env-guard-script`；流程已固定环境时 `codebaby fmt --fix` 自动删掉这段脚本。
