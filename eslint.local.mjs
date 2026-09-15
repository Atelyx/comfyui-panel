/**
 * 本地校验用的 ESLint 配置（不参与插件运行时）。
 * 解析器与 react-hooks 插件借自同机宿主仓库的工具链：仓库根默认按目录布局推断，
 * 可用环境变量 ATELYX_ROOT 覆盖。
 */
import { readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import path from "node:path";
import process from "node:process";

const hostRoot = process.env.ATELYX_ROOT ?? fileURLToPath(new URL("../../Atelyx/", import.meta.url));

/** 从宿主仓库加载包；宿主用 pnpm，顶层只链接直接依赖，传递依赖回退到 .pnpm 存储取最高版本。 */
function loadHostModule(name) {
  const require = createRequire(path.join(hostRoot, "package.json"));
  try {
    return require(name);
  } catch {
    const storeDir = path.join(hostRoot, "node_modules", ".pnpm");
    const prefix = name.replace("/", "+") + "@";
    const latest = readdirSync(storeDir)
      .filter((entry) => entry.startsWith(prefix))
      .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }))
      .at(-1);
    if (!latest) throw new Error(`ESLint 配置初始化失败：宿主仓库中找不到 ${name}（可设置环境变量 ATELYX_ROOT）`);
    const pkgDir = path.join(storeDir, latest, "node_modules", name);
    return createRequire(path.join(pkgDir, "package.json"))(name);
  }
}

const parser = loadHostModule("@typescript-eslint/parser");
const reactHooks = loadHostModule("eslint-plugin-react-hooks");

export default [
  {
    files: ["src/**/*.ts", "src/**/*.tsx"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      parser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    plugins: {
      "react-hooks": reactHooks.default ?? reactHooks,
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "no-dupe-keys": "error",
    },
  },
];
