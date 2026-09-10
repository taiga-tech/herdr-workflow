#!/usr/bin/env python3
"""Check this documentation package using only the Python standard library.

Checks document metadata, relative links and explicit anchors, source snapshots,
migration entries, and preserved acceptance criteria. It does not run Rust,
contact Herdr, execute workflow commands, or validate the YAML schema.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]


def read_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"Expected a JSON object: {path}")
    return value


def strip_front_matter(text: str) -> str:
    if text.startswith("---\n"):
        parts = text.split("\n---\n", 1)
        return parts[1] if len(parts) == 2 else text
    return text


def without_fences(text: str) -> tuple[str, bool]:
    lines = []
    opening = None
    for line in text.splitlines():
        match = re.match(r"^\s{0,3}(`{3,}|~{3,})", line)
        if match:
            token = match.group(1)
            if opening is None:
                opening = token
            elif token[0] == opening[0] and len(token) >= len(opening):
                opening = None
            continue
        if opening is None:
            lines.append(line)
    return "\n".join(lines), opening is None


def explicit_anchors(text: str) -> list[str]:
    return re.findall(r'<a\s+(?:id|name)="([^"]+)"\s*>', text)


def within_root(path: Path) -> bool:
    try:
        path.resolve().relative_to(ROOT)
        return True
    except ValueError:
        return False


def normalize_table_cell(text: str) -> str:
    text = re.sub(r'<a\s+(?:id|name)="[^"]+"\s*>\s*</a>', '', text)
    return re.sub(r"\s+", " ", text).strip()


def acceptance_rows(text: str) -> dict[str, tuple[str, str]]:
    rows = {}
    for line in text.splitlines():
        cells = [normalize_table_cell(c) for c in line.strip().strip('|').split('|')]
        if len(cells) == 3 and re.fullmatch(r'T\d{2}', cells[0]):
            rows[cells[0]] = (cells[1], cells[2])
    return rows


def check(check_migration: bool = False) -> dict:
    errors = []
    counts = {}
    manifest = read_json(ROOT / 'docs/manifest.json')
    records = manifest.get('documents', [])
    by_path = {}
    ids = set()
    texts = {}
    anchors = {}
    edges = {}
    valid_status = {'draft','proposed','accepted','current','reference','redirect','archived','superseded','rejected'}
    for record in records:
        doc_id, rel = record.get('id'), record.get('path')
        if not doc_id or doc_id in ids:
            errors.append(f'Duplicate or missing document ID: {doc_id}')
        ids.add(doc_id)
        if not rel or rel in by_path:
            errors.append(f'Duplicate or missing document path: {rel}')
            continue
        by_path[rel] = record
        path = ROOT / rel
        if not within_root(path) or not path.is_file():
            errors.append(f'Missing or out-of-root document: {rel}')
            continue
        text = path.read_text(encoding='utf-8')
        if not text.startswith('---\n') or '\n---\n' not in text:
            errors.append(f'Missing front matter: {rel}')
            continue
        front = text.split('\n---\n',1)[0]
        actual_id = re.search(r'^id:\s*(\S+)\s*$',front,re.M)
        actual_status = re.search(r'^status:\s*(\S+)\s*$',front,re.M)
        if not actual_id or actual_id.group(1) != doc_id:
            errors.append(f'Document ID mismatch: {rel}')
        if not actual_status or actual_status.group(1) != record.get('status'):
            errors.append(f'Document status mismatch: {rel}')
        if record.get('status') not in valid_status:
            errors.append(f'Unknown status: {rel}')
        for key in ['title','documentVersion','updated']:
            if not re.search(rf'^{key}:\s*\S+',front,re.M):
                errors.append(f'Missing metadata {key}: {rel}')
        body, closed = without_fences(strip_front_matter(text))
        if not closed:
            errors.append(f'Unclosed fenced code block: {rel}')
        if len(re.findall(r'^#\s+\S',body,re.M)) != 1:
            errors.append(f'Expected exactly one H1: {rel}')
        extracted = explicit_anchors(body)
        if len(extracted) != len(set(extracted)):
            errors.append(f'Duplicate explicit anchor: {rel}')
        texts[rel], anchors[rel], edges[rel] = body, set(extracted), set()

    actual_files = {p.relative_to(ROOT).as_posix() for p in ROOT.rglob('*.md')}
    # The immutable source document is intentionally not a current document.
    actual_files.discard('archive/design-0.1.md')
    if actual_files != set(by_path):
        errors.append(f'Manifest file mismatch: unlisted={sorted(actual_files-set(by_path))}; missing={sorted(set(by_path)-actual_files)}')
    links = 0
    for rel, body in texts.items():
        # Inline code examples are not navigable links.
        body = re.sub(r'`[^`\n]*`','',body)
        for match in re.finditer(r'\[[^\]\n]+\]\(([^\s)]+)\)',body):
            target = match.group(1)
            parsed = urlsplit(target)
            if parsed.scheme or parsed.netloc:
                continue
            links += 1
            path = (ROOT/rel).parent / unquote(parsed.path) if parsed.path else ROOT/rel
            path = path.resolve()
            if not within_root(path):
                errors.append(f'Link leaves package: {rel} -> {target}')
                continue
            if not path.exists():
                errors.append(f'Broken relative link: {rel} -> {target}')
                continue
            dest = path.relative_to(ROOT).as_posix()
            if dest in by_path:
                edges[rel].add(dest)
            if parsed.fragment and path.suffix == '.md':
                fragment = unquote(parsed.fragment)
                if dest not in anchors:
                    other, _ = without_fences(strip_front_matter(path.read_text(encoding='utf-8')))
                    anchors[dest] = set(explicit_anchors(other))
                if fragment not in anchors[dest]:
                    errors.append(f'Missing explicit anchor: {rel} -> {target}')
    reachable, todo = set(), ['README.md']
    while todo:
        rel = todo.pop()
        if rel in reachable:
            continue
        reachable.add(rel)
        todo.extend(edges.get(rel,set())-reachable)
    orphans = set(by_path)-reachable
    if orphans:
        errors.append(f'Documents unreachable from README: {sorted(orphans)}')
    counts['activeMarkdownDocuments'] = len(by_path)
    counts['relativeLinks'] = links
    counts['explicitAnchors'] = sum(len(v) for k,v in anchors.items() if k in by_path)
    counts['reachableDocuments'] = len(reachable & set(by_path))

    source_manifest = read_json(ROOT/'archive/source-manifest.json')
    for item in source_manifest.get('files',[]):
        p = ROOT/item['path']
        if not within_root(p) or not p.is_file():
            errors.append(f'Missing source snapshot: {item["path"]}')
            continue
        actual = hashlib.sha256(p.read_bytes()).hexdigest()
        if actual != item['sha256']:
            errors.append(f'Source snapshot modified: {item["path"]}')
    counts['sourceSnapshots'] = len(source_manifest.get('files',[]))

    migration = read_json(ROOT/'docs/meta/migration.json')
    entries = migration.get('sections',{})
    expected_sections = {str(i) for i in range(1,28)} | {'appendix'}
    if set(entries) != expected_sections:
        errors.append('Migration map must cover source sections 1..27 and appendix')
    for key, item in entries.items():
        dest, anchor = item.get('path'), item.get('anchor')
        if dest not in by_path or anchor not in anchors.get(dest,set()):
            errors.append(f'Invalid migration destination for source section {key}')
    counts['mappedSourceSections'] = len(entries)

    original = (ROOT/'archive/design-0.1.md').read_text(encoding='utf-8')
    current = (ROOT/'docs/testing/acceptance.md').read_text(encoding='utf-8')
    old_rows, new_rows = acceptance_rows(original), acceptance_rows(current)
    expected_tests = {f'T{i:02d}' for i in range(1,33)}
    if set(old_rows) != expected_tests:
        errors.append('Unexpected source acceptance test IDs')
    if not expected_tests.issubset(new_rows):
        errors.append('A source acceptance test ID is missing')
    # Future criteria may evolve. Report drift against the snapshot for review.
    for test in sorted(expected_tests & set(new_rows)):
        if check_migration and new_rows[test] != old_rows[test]:
            errors.append(f'Acceptance criterion differs from source: {test}; record an intentional migration change before updating the preservation check')
    for test in expected_tests:
        if test.lower() not in anchors.get('docs/testing/acceptance.md',set()):
            errors.append(f'Acceptance test has no stable anchor: {test}')
    features_text = texts.get('docs/features.md','')
    for test in expected_tests:
        if f'#{test.lower()}' not in features_text:
            errors.append(f'Acceptance test is not linked from a feature: {test}')
    counts['sourceAcceptanceIdsRetained'] = len(expected_tests & set(new_rows))
    counts['unchangedSourceAcceptanceCriteria'] = sum(new_rows.get(t) == old_rows[t] for t in expected_tests)
    counts['featureIds'] = len([a for a in anchors.get('docs/features.md',set()) if re.fullmatch(r'f\d{2}',a)])
    counts['decisionRecords'] = len([r for r in records if re.fullmatch(r'ADR-\d{4}',r['id'])])
    counts['openQuestionIds'] = len([a for a in anchors.get('docs/planning/open-questions.md',set()) if re.fullmatch(r'q\d{2}',a)])
    return {'ok':not errors,'counts':counts,'errors':errors,'scope':'Documentation structure and preserved source material only. No product execution or external network checks.'}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--json',action='store_true',help='Print a machine-readable result')
    parser.add_argument('--check-migration',action='store_true',help='Also require exact acceptance-criterion text from the initial source snapshot')
    args = parser.parse_args()
    try:
        result = check(check_migration=args.check_migration)
    except (OSError, ValueError, KeyError, TypeError) as exc:
        result = {'ok':False,'counts':{},'errors':[str(exc)]}
    if args.json:
        print(json.dumps(result,ensure_ascii=False,indent=2))
    else:
        print('PASS' if result['ok'] else 'FAIL')
        for key, value in result['counts'].items():
            print(f'  {key}: {value}')
        for error in result['errors']:
            print(f'  ERROR: {error}',file=sys.stderr)
    return 0 if result['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())