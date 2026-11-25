#!/bin/bash
# Crea un repositorio de demostración determinista para capturas y pruebas
# manuales: historial con ramas y merges, tags, stash, remoto bare y cambios
# locales (staged, unstaged, sin seguimiento, borrado).
# Uso: make-fixture.sh <directorio-destino>   → crea <destino>/origin.git y <destino>/demo
set -euo pipefail
ROOT="${1:?directorio destino}"
rm -rf "$ROOT"; mkdir -p "$ROOT"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export GIT_AUTHOR_NAME="Ana Torres" GIT_AUTHOR_EMAIL="ana@example.com"
export GIT_COMMITTER_NAME="Ana Torres" GIT_COMMITTER_EMAIL="ana@example.com"
DAY=0
commit() { # commit <mensaje> [autor]
  DAY=$((DAY + 1))
  local stamp; stamp=$(printf '2026-09-%02dT10:%02d:00+00:00' $((DAY / 3 + 1)) $((DAY * 7 % 60)))
  local name="${2:-Ana Torres}"; local mail; mail="$(echo "$name" | tr 'A-Z ' 'a-z.')@example.com"
  git add --all
  GIT_AUTHOR_DATE="$stamp" GIT_COMMITTER_DATE="$stamp" GIT_AUTHOR_NAME="$name" GIT_AUTHOR_EMAIL="$mail" \
    git commit -q -m "$1"
}
git init -q --bare -b main "$ROOT/origin.git"
git init -q -b main "$ROOT/demo"; cd "$ROOT/demo"
git config user.name "Ana Torres"; git config user.email "ana@example.com"; git config commit.gpgsign false

mkdir -p src/lib docs styles
cat > README.md <<'X'
# Demo project

A small project used to exercise the Git client.

## Getting started

Run `npm install` and then `npm start`.
X
cat > src/app.js <<'X'
import { formatDate } from './lib/util.js'

export function main() {
  const today = formatDate(new Date())
  console.log(`Started on ${today}`)
  return 0
}

main()
X
cat > src/lib/util.js <<'X'
export function formatDate(date) {
  return date.toISOString().slice(0, 10)
}
X
printf 'body {\n  margin: 0;\n  font-family: sans-serif;\n}\n' > styles/main.css
printf '# Guide\n\nFirst steps with the project.\n' > docs/guide.md
printf '# Old notes\n\nTo be removed.\n' > docs/old.md
commit "Initial commit"
printf '\nexport function clamp(value, min, max) {\n  return Math.min(max, Math.max(min, value))\n}\n' >> src/lib/util.js
commit "Add clamp helper"
printf '\n## License\n\nMIT\n' >> README.md
commit "Document the license" "Luis Vega"
git tag -a v1.0.0 -m "First stable release"

git checkout -q -b feature/login
mkdir -p src/auth
printf 'export function login(user, password) {\n  if (!user || !password) {\n    throw new Error("Missing credentials")\n  }\n  return { user, token: "demo" }\n}\n' > src/auth/login.js
commit "Add login module" "Luis Vega"
printf 'export function logout(session) {\n  session.token = null\n}\n' > src/auth/logout.js
commit "Add logout" "Luis Vega"
sed -i 's/throw new Error("Missing credentials")/throw new Error("Missing user or password")/' src/auth/login.js
commit "Clarify the login error message" "Luis Vega"

git checkout -q main
printf '.button {\n  padding: 4px 12px;\n  border-radius: 6px;\n}\n' >> styles/main.css
commit "Style the buttons"
GIT_AUTHOR_DATE="2026-09-05T09:00:00+00:00" GIT_COMMITTER_DATE="2026-09-05T09:00:00+00:00" \
  git merge -q --no-ff feature/login -m "Merge branch 'feature/login'"
git tag v1.1.0

git checkout -q -b feature/search HEAD~1
printf 'export function search(items, term) {\n  return items.filter(item => item.includes(term))\n}\n' > src/lib/search.js
commit "Add search helper" "Marta Ruiz"
printf '\nexport function searchCaseInsensitive(items, term) {\n  const needle = term.toLowerCase()\n  return items.filter(item => item.toLowerCase().includes(needle))\n}\n' >> src/lib/search.js
commit "Search ignoring case" "Marta Ruiz"

git checkout -q -b hotfix/crash main
sed -i 's/return 0/return process.exitCode ?? 0/' src/app.js
commit "Fix crash when exit code is undefined"

git checkout -q main
printf '\n## Usage\n\nImport `main` from `src/app.js`.\n' >> docs/guide.md
commit "Explain usage in the guide"
git remote add origin "$ROOT/origin.git"
git push -q -u origin main
git push -q origin feature/login v1.0.0
git push -q origin main:release/1.x
printf '\nexport const VERSION = "1.2.0"\n' >> src/lib/util.js
commit "Expose the version constant"
sed -i 's/Started on/Application started on/' src/app.js
commit "Reword the startup message"

# Stash
printf '\n// TODO: experiment\n' >> src/lib/util.js
git stash push -q -m "WIP: experiment with util"

# Cambios locales
sed -i 's/const today = formatDate(new Date())/const today = formatDate(new Date())\n  const mode = process.env.NODE_ENV || "development"/' src/app.js
sed -i 's/console.log(`Application started on ${today}`)/console.log(`Application started on ${today} (${mode})`)/' src/app.js
printf 'export function featureFlag(name) {\n  return process.env[`FLAG_${name}`] === "1"\n}\n' > src/new-feature.js
git add src/new-feature.js
printf '\nSee the guide in `docs/guide.md`.\n' >> README.md
git add README.md
rm docs/old.md
printf 'Remember to update the changelog.\n' > notes.txt
echo "fixture listo en $ROOT/demo"
