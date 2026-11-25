#!/usr/bin/env python3
"""Genera los manifiestos de actualización que lee el actualizador de Mergeon.

Mismo formato que publica electron-builder (`latest.yml`, `latest-linux.yml`,
`latest-mac.yml`): lo entienden tanto `mergeon_updater::parse_update_manifest`
(versión nativa) como electron-updater (versión Electron).

    version: 0.9.1
    files:
      - url: Mergeon-0.9.1.AppImage
        sha512: <sha512 del archivo en base64>
        size: 123456
    path: Mergeon-0.9.1.AppImage
    sha512: <el del primer archivo>
    releaseDate: '2026-09-30T18:21:04.512Z'

Uso (solo biblioteca estándar, Python 3.8+):

    update-manifest.py --platform linux   --version 0.9.1 --dir native/dist
    update-manifest.py --platform windows --version 0.9.1 --dir native/dist
    update-manifest.py --platform mac     --version 0.9.1 --dir native/dist
    update-manifest.py --version 0.9.1 --output feed/latest.yml archivo...

Con `--platform` elige los artefactos de esa versión en `--dir` por su nombre
y escribe ahí el manifiesto con el nombre del canal. Con archivos explícitos,
el primero es el principal (`path` / `sha512`).
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

CHANNEL = "latest"

# Escalares que YAML leería como otra cosa si van sin comillas.
_PLAIN_SAFE = re.compile(r"^[A-Za-z0-9_./+=-]+$")
_AMBIGUOUS = re.compile(
    r"^(~|null|true|false|yes|no|on|off|[-+]?(\d[\d_]*)?\.?\d*([eE][-+]?\d+)?|0x[0-9a-fA-F]+)$",
    re.IGNORECASE,
)


def yaml_scalar(value: str) -> str:
    """Escalar plano si es inequívoco; si no, entre comillas simples."""
    if _PLAIN_SAFE.match(value) and not _AMBIGUOUS.match(value) and value[0] not in "-=":
        return value
    return "'" + value.replace("'", "''") + "'"


def sha512_base64(path: Path) -> str:
    digest = hashlib.sha512()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return base64.b64encode(digest.digest()).decode("ascii")


def release_date(explicit: str | None) -> str:
    """Fecha ISO 8601 en UTC con milisegundos, como `Date.toISOString()`."""
    if explicit:
        return explicit
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    moment = (
        datetime.fromtimestamp(int(epoch), tz=timezone.utc)
        if epoch
        else datetime.now(tz=timezone.utc)
    )
    return moment.strftime("%Y-%m-%dT%H:%M:%S.") + f"{moment.microsecond // 1000:03d}Z"


def render_manifest(version: str, files: list[Path], date: str) -> str:
    if not files:
        raise ValueError("el manifiesto necesita al menos un archivo")
    entries = [(path.name, sha512_base64(path), path.stat().st_size) for path in files]
    lines = [f"version: {yaml_scalar(version)}", "files:"]
    for name, sha512, size in entries:
        lines += [
            f"  - url: {yaml_scalar(name)}",
            f"    sha512: {sha512}",
            f"    size: {size}",
        ]
    primary_name, primary_sha512, _ = entries[0]
    lines += [
        f"path: {yaml_scalar(primary_name)}",
        f"sha512: {primary_sha512}",
        f"releaseDate: '{date}'",
    ]
    return "\n".join(lines) + "\n"


def linux_arch_suffix(name: str) -> str:
    """`latest-linux.yml` es x64; las demás arquitecturas llevan sufijo."""
    return "-arm64" if re.search(r"(arm64|aarch64)", name) else ""


def platform_plan(platform: str, version: str, directory: Path) -> dict[str, list[Path]]:
    """Qué manifiestos escribir y con qué archivos, según los nombres de `dist/`."""
    present = sorted(path for path in directory.iterdir() if path.is_file())
    # La versión exacta: "0.9.1" no debe arrastrar los artefactos de "0.9.10".
    exact = re.compile(r"(?<![0-9.])" + re.escape(version) + r"(?![0-9])")
    versioned = [path for path in present if exact.search(path.name)]

    if platform == "windows":
        installers = [p for p in versioned if p.name.lower().endswith(".exe")]
        return {f"{CHANNEL}.yml": installers}

    if platform == "mac":
        zips = [p for p in versioned if p.name.endswith("-mac.zip")]
        # El actualizador elige por arquitectura (los nombres con "arm64"
        # solo los toma Apple Silicon); el principal es el primero.
        zips.sort(key=lambda p: ("arm64" in p.name, p.name))
        return {f"{CHANNEL}-mac.yml": zips}

    if platform == "linux":
        plan: dict[str, list[Path]] = {}
        packages = [p for p in versioned if p.name.endswith((".AppImage", ".deb"))]
        for package in packages:
            plan.setdefault(f"{CHANNEL}-linux{linux_arch_suffix(package.name)}.yml", []).append(package)
        for files in plan.values():
            # El AppImage primero: es el que se actualiza solo.
            files.sort(key=lambda p: (not p.name.endswith(".AppImage"), p.name))
        return plan

    raise ValueError(f"plataforma desconocida: {platform}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Genera latest*.yml (formato electron-builder) para el actualizador de Mergeon."
    )
    parser.add_argument("--version", required=True, help="versión publicada (la del binario)")
    parser.add_argument("--platform", choices=["linux", "windows", "mac"])
    parser.add_argument("--dir", type=Path, help="directorio con los artefactos (con --platform)")
    parser.add_argument("--output", type=Path, help="manifiesto a escribir (con archivos explícitos)")
    parser.add_argument("--release-date", help="fecha ISO 8601; por defecto ahora (o SOURCE_DATE_EPOCH)")
    parser.add_argument("files", nargs="*", type=Path, help="artefactos; el primero es el principal")
    args = parser.parse_args(argv)

    date = release_date(args.release_date)

    if args.platform:
        if args.files or args.output:
            parser.error("--platform no se combina con archivos explícitos ni --output")
        if not args.dir:
            parser.error("--platform necesita --dir")
        plan = platform_plan(args.platform, args.version, args.dir)
        plan = {name: files for name, files in plan.items() if files}
        if not plan:
            print(
                f"error: no hay artefactos de {args.platform} {args.version} en {args.dir}",
                file=sys.stderr,
            )
            return 1
        outputs = {args.dir / name: files for name, files in plan.items()}
    else:
        if not args.files or not args.output:
            parser.error("indica --platform/--dir, o --output y al menos un archivo")
        outputs = {args.output: args.files}

    for output, files in outputs.items():
        for path in files:
            if not path.is_file():
                print(f"error: no existe {path}", file=sys.stderr)
                return 1
        output.parent.mkdir(parents=True, exist_ok=True)
        # newline="\n": también en Windows el manifiesto sale con saltos LF.
        with output.open("w", encoding="utf-8", newline="\n") as handle:
            handle.write(render_manifest(args.version, files, date))
        print(f"{output}: {', '.join(path.name for path in files)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
