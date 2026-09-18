# Development Specification Document

DSCode desktop (Tauri 2 + React 19), embeds the Harness UI served at `http://127.0.0.1:3080`.

- **端口隔离**：release 默认 `3080`，debug（`pnpm tauri dev` / `cargo build`）默认 `3081`，由 `config::setting::default_port()` 用 `cfg!(debug_assertions)` 区分，避免开发时与已运行的桌面端争用端口。
- **离线内置运行时**：Windows x64 安装器按 perMachine 安装到 Program Files。Node、DSH、MinGit、PowerShell 在 `resources/bundle`，界面使用系统 WebView2；由 `scripts/prepare-runtime.ts` 在构建机准备，版本与摘要锁定在 `scripts/runtime`。用户电脑不执行 npm 安装、不下载或切换核心；更新只能运行完整桌面安装包。
- **数据隔离**：程序资源只读，设置/日志仍在用户 AppData，release `$DSH_HOME` 默认为 `~/.dsh`，debug 为 `~/.dsh.dev` 且 store 为 `.store.dev.dat`。debug 读取源码 `src-tauri/resources/bundle`，不修改生产资源或用户 PATH。
- **上游插件边界**：桌面端不提供插件市场、预装、升级、卸载或修复功能，只保留 DeepSeek 官方 Release 对应的原生上游插件。

- Prioritize using customized components from src/components, hero-ui.
- This will help minimize the need for writing custom classes.
- If you write new content, you need to handle i18n en keys
- i18n keys must be flat (no nesting), use dot-notation flat keys only
- No hardcoded strings; sync i18n locale files (`src/i18n/locales/en-US.json` / `zh-CN.json`)
- If the component you write/modify is too complex, you need to split it into multiple components
- Repeated logic should be encapsulated into methods/components

## Tech Stack

- **Frontend**: React 19 + TS + Tailwind 4 (no plain CSS), Vite (`src/`)
- **Backend**: Rust / Tauri 2 (`src-tauri/src/`)
  - `bridge/cmd.rs`: Tauri commands (register in `lib.rs` `generate_handler!`)
  - `config/`: constants, paths (`runtime.rs`), settings (`setting.rs`), i18n & theme
  - `scripts/prepare-runtime.ts`: build-time offline runtime preparation
  - `service/workflow/`: process lifecycle (Windows no-window: `win_spawn.rs`)
  - `service/cli/`: `dsh`/`pnpm` shims + PATH registration (`mod.rs`/`shim.rs`/`path.rs`/`core.rs`)
  - `service/scheduler/` + `task/`: health check & polling

## Dev Commands

```bash
pnpm install && pnpm dev    # frontend dev
pnpm typecheck              # frontend TS check (must run after frontend changes)
pnpm tauri dev              # full desktop debug
cargo check && cargo test   # Rust check & unit tests (run in src-tauri)
```

## Basics

- No `useCallback` / `useMemo` — `react-compiler` 已通过 Vite 接入（`babel-plugin-react-compiler`，target 19）用于自动记忆化；
- Component functions use `function` declaration; inline events/callbacks use arrow functions

## Function Declaration Specification

- **Named functions must use `function` declaration, not arrow functions**
- **Arrow functions can only be used when passed as callback parameters**

```tsx
// ✅ Correct
function Component() {
  function handleClick() {
    console.log('click');
  }
  return <button onClick={handleClick}>Click</button>;
}

// ✅ Correct: Arrow functions can be used for callbacks
useQuery({
  queryFn: async () => {
    return fetchData();
  },
});
```

## Data Processing Specification

### Use directly in pages

**Use case:** When data doesn't need additional processing after fetching

```tsx
function MyPage() {
  const { data } = useQuery({
    queryKey: ['simple-data'],
    queryFn: () => fetchData(),
  });
}
```

### Create files in services directory

**Use case:** Backend type error handling, parameter processing, composite requests, polling, data caching, etc.

**File naming:** `use-get-{resource}.ts`, `use-post-{resource}.ts`, `use-put-{resource}.ts`, `use-delete-{resource}.ts`

```tsx
// services/use-get-exchange-rates.ts
export function useGetExchangeRates(params) {
  return useQuery({
    queryKey: [getApiExchangeRatesCurrencyPair.name, params],
    queryFn: () => getApiExchangeRatesCurrencyPair(params).then(res => res.data?.data),
  });
}
```

## Conditional Rendering Specification

Use `If`, `Then`, `Else` components by `react-if-lite` package instead of ternary operators and `&&` operators

```tsx
// Basic usage
<If cond={!isLoading} else={<LoadingSpinner />}>
  <Content />
</If>

// Simple condition: use props
<If cond={isBasic} then={<GrayZuanIcon />} else={<ZuanIcon />} />

// Complex condition: use child components
<If cond={hasData}>
  <Then>
    <DataTable data={data} />
  </Then>
  <Else>
    <Empty />
  </Else>
</If>

// Specify render tag
<If cond={condition} as="div">
  {content}
</If>
```

## State Management Specification

### Multiple module pages shared data: defineStore

```tsx
// store/modules/user.ts
export const user = defineStore({
  state: () => ({ user: null }),
  actions: { async fetchUser() { ... } },
});

// Usage
import { store } from '@/store';
const { user } = useStore(store.user);
```

## Figma → Code

1. **Use theme tokens, not hardcoded colors** — `text-warning` over `text-[#7A5E38]`
2. **Component rules serve the design** — override styles when defaults don't match
3. **Structure is style** — map Figma frames directly to component tree, translate gap/padding directly
4. **Use component APIs** — express states via `value`, `size`, `variant` props instead of hand-writing styles

## tv Usage

Use `tv` when a component has multiple style variants/slots.

- `slots` defines all style areas; `variants` only writes changing styles
- Derive variant types via `VariantProps<typeof tvConfig>['variant']`

```tsx
export const dialog = tv({
  slots: {
    base: 'relative',
    icon: 'size-14 items-center justify-center rounded-full',
    iconContent: 'size-[22px]',
  },
  variants: {
    variant: {
      success: { icon: 'bg-[#E7EFE3]' },
      warning: { icon: 'bg-[#F5EAD3]' },
    },
  },
  defaultVariants: { variant: 'default' },
})
// Use: const { icon } = dialog({ variant })
```

## Overlastic Dialog Pattern (`@overlastic/react`)

**Use case:** Imperative dialog (confirm, PIN, KYC).

```
Hook       → useOverlay(Component) returns an async opener function
_layout.tsx → mount OverlaysProvider at root
Component  → render actual UI with useDisclosure
```

```tsx
// usage — resolves with the confirm value
const { foo } = useOverlay(FooComponent)
const result = await foo(options)
```

```tsx
// _layout.tsx — mount once at root
<OverlaysProvider>
  <App />
</OverlaysProvider>

// components/foo.tsx — render actual UI
import type { PropsWithOverlays } from '@overlastic/react'
import { useDisclosure } from '@overlastic/react'

export interface FooProps extends PropsWithOverlays, FooOptions { ... }

export function FooComponent(props: FooProps) {
  const disclosure = useDisclosure({ props, delay: 300 })
  return (
    <BottomSheet isOpen={disclosure.visible} onOpenChange={() => disclosure.cancel()}>
      {/* ... */}
      <Button onPress={() => disclosure.confirm(value)} />
    </BottomSheet>
  )
}
```

- Props are flat options; `PropsWithOverlays<Payload, Result>` types the payload and the promise result
- `disclosure.confirm(value?)` resolves the opener's promise, `disclosure.cancel()` closes without a result
- Component props/result types live in the component file, the hook imports them from there

## Backend Rules (Rust / Tauri)

1. **Comments**: Chinese only; `//!` for module headers, `///` for functions (focus on "why").
2. **Errors/Logs**: `Result<_, String>` errors need an uppercase prefix (e.g. `NODE_NOT_FOUND: ...`); log key paths.
3. **Settings**: new `Setting` fields need `#[serde(default...)]` and export in `config/mod.rs`.
4. **Windows**:
   - Spawn children with `CREATE_NO_WINDOW (0x08000000)`.
   - Kill the process tree when stopping services (`taskkill /T /F`) to avoid DLL lock on update.
   - Broadcast `WM_SETTINGCHANGE` after writing PATH; tell users to reopen terminals.
5. **CLI shim (`service/cli`)**:
   - `resources/bundle/bin/dsh.cmd` 在构建时生成，安装器统一维护。
   - 固定调用内置 Node 和 DSH；不再探测或更新系统 Node/npm/pnpm。
   - 运行时开关仅注册/注销用户 PATH，不创建、删除或覆盖 Program Files 中的文件。
   - Shim text must be English-only (cmd parses by code page).
6. **Cross-platform/Tests**: Unix-only code gets `#[cfg_attr(windows, allow(dead_code))]`; unit tests in `#[cfg(test)] mod tests`, skip gracefully when restricted.
7. **Deps/Docs**: no heavy deps, prefer existing `windows-sys`; README minimal, en/zh synced.

## Pitfalls

- `dsh` CLI 入口为 `resources/bundle/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js`；Node 为 `resources/bundle/node/node.exe`。
- 运行时只读；AppData 仅存 `.store.dat` / `.store.dev.dat`、日志等用户数据，`$DSH_HOME` 在用户主目录（release `~/.dsh`，debug `~/.dsh.dev`）。
- Service args: `node bin.js --profile web --host 127.0.0.1 --port <setting.port>`; CLI PATH registration is optional.

## Summary

- **API Import**: 直接 `import { invoke } from '@tauri-apps/api/core'`（本仓库没有 `@/apis` 层）；类型就近定义在 hooks/组件文件中
- **Function Declaration**: Use `function`, not arrow functions
- **Conditional Rendering**: Use `If`, `Then`, `Else` components instead of ternary operators and `&&` operators
- **Data Processing**: Simple scenarios use `useQuery`/`useMutation` directly, complex scenarios create service files
- **State Management**: Multi-module shared state uses `defineStore + useStore`（valtio-define）；`defineScope`/`useScope` 已弃用（无引用），新代码勿用
