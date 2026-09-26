import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const themes = ["lara-light-indigo", "lara-dark-indigo"];

mkdirSync(join(root, "public", "themes"), { recursive: true });

for (const name of themes) {
  const src = join(root, "node_modules", "primereact", "resources", "themes", name, "theme.css");
  const dst = join(root, "public", "themes", `${name}.css`);
  copyFileSync(src, dst);
  console.log(`copied ${name}.css`);
}
