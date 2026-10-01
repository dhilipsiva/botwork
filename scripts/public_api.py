"""List the crate's public Rust API from rustdoc's JSON output.

Usage: public_api.py RUSTDOC_JSON
       public_api.py --leaks RUSTDOC_JSON

Every item reachable through a public path is listed once per path, with its
public fields, variants, inherent methods, and the traits it implements;
`#[doc(hidden)]` items are left out, since they are not part of the API. The
listing is sorted, so a checked-in copy shows any change to the surface as a
diff.

With --leaks, list instead each listed item whose public signature names a
hidden item of this crate, and exit with status 1 if there is one: such an
item would tie the API to something outside it.
"""
import json
import sys

HIDDEN = "#[doc(hidden)]"


def hidden(item):
    return any(HIDDEN in json.dumps(attr) for attr in item.get("attrs", []))


def describe(index, path, item, lines, exposed):
    exposed.add(item["id"])
    kind, inner = next(iter(item["inner"].items()))
    flags = " #[non_exhaustive]" if any("non_exhaustive" in json.dumps(attr) for attr in item.get("attrs", [])) else ""
    if kind == "function":
        lines.add(f"fn {path}")
    elif kind == "constant":
        lines.add(f"const {path}")
    elif kind == "static":
        lines.add(f"static {path}")
    elif kind == "type_alias":
        lines.add(f"type {path}")
    elif kind == "macro":
        lines.add(f"macro {path}")
    elif kind == "trait":
        lines.add(f"trait {path}")
        for member in inner["items"]:
            child = index[str(member)]
            if not hidden(child):
                lines.add(f"trait-item {path}::{child['name']}")
    elif kind in ("struct", "enum", "union"):
        lines.add(f"{kind} {path}{flags}")
        if kind == "struct":
            shape = inner["kind"]
            fields = shape.get("plain", {}).get("fields", []) if isinstance(shape, dict) else []
            if isinstance(shape, dict) and "tuple" in shape:
                fields = [field for field in shape["tuple"] if field is not None]
            for field in fields:
                child = index[str(field)]
                if child.get("visibility") == "public" and not hidden(child):
                    lines.add(f"field {path}::{child['name']}")
        if kind == "enum":
            for variant in inner["variants"]:
                child = index[str(variant)]
                if not hidden(child):
                    lines.add(f"variant {path}::{child['name']}")
        for implementation in inner["impls"]:
            block = index[str(implementation)]["inner"]["impl"]
            if block["is_synthetic"] or block["blanket_impl"] is not None:
                continue
            if block["trait"] is None:
                for member in block["items"]:
                    child = index[str(member)]
                    if child.get("visibility") == "public" and not hidden(child):
                        member_kind = next(iter(child["inner"]))
                        lines.add(f"{'fn' if member_kind == 'function' else member_kind} {path}::{child['name']}")
            else:
                lines.add(f"impl {block['trait']['path']} for {path}")
    else:
        lines.add(f"{kind} {path}")


def walk(index, module_id, prefix, lines, seen, exposed):
    module = index[str(module_id)]
    for member in module["inner"]["module"]["items"]:
        item = index[str(member)]
        if item.get("visibility") != "public" or hidden(item):
            continue
        kind, inner = next(iter(item["inner"].items()))
        if kind == "use":
            target = inner.get("id")
            name = inner["name"]
            if target is None or str(target) not in index:
                lines.add(f"reexport {prefix}::{name} = {inner['source']}")
                continue
            if inner.get("is_glob"):
                walk(index, target, prefix, lines, seen, exposed)
                continue
            item = index[str(target)]
            kind = next(iter(item["inner"]))
            path = f"{prefix}::{name}"
        else:
            path = f"{prefix}::{item['name']}"
        if kind == "module":
            if (member, path) in seen:
                continue
            seen.add((member, path))
            lines.add(f"mod {path}")
            walk(index, item["id"], path, lines, seen, exposed)
        else:
            describe(index, path, item, lines, exposed)


def surface(document):
    """The listing's lines, and the ids of the items it lists."""
    index = document["index"]
    root = index[str(document["root"])]
    lines, exposed = set(), set()
    walk(index, document["root"], root["name"], lines, set(), exposed)
    return sorted(lines), exposed


def listing(document):
    return surface(document)[0]


def referenced(node, found):
    """Collect the ids of the items a signature names."""
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "resolved_path" and isinstance(value, dict) and "id" in value:
                found.add(value["id"])
            referenced(value, found)
    elif isinstance(node, list):
        for value in node:
            referenced(value, found)
    return found


def signatures(index, item):
    """The public parts of an item that other code sees: field and variant
    types, function signatures, and the signatures of inherent methods."""
    kind, inner = next(iter(item["inner"].items()))
    parts = []
    if kind == "struct":
        shape = inner["kind"]
        fields = shape.get("plain", {}).get("fields", []) if isinstance(shape, dict) else []
        if isinstance(shape, dict) and "tuple" in shape:
            fields = [field for field in shape["tuple"] if field is not None]
        parts += [index[str(field)] for field in fields if index[str(field)].get("visibility") == "public"]
    if kind == "enum":
        # Variants take their enum's visibility, which rustdoc calls "default".
        parts += [index[str(variant)] for variant in inner["variants"]]
    if kind in ("struct", "enum"):
        for implementation in inner["impls"]:
            block = index[str(implementation)]["inner"]["impl"]
            if block["trait"] is None and not block["is_synthetic"]:
                parts += [index[str(member)] for member in block["items"] if index[str(member)].get("visibility") == "public"]
        return [part for part in parts if not hidden(part)]
    return [item]


def leaks(document):
    """Listed items whose signatures name an item of this crate that the
    listing does not expose: rustdoc leaves hidden items out of its output, so
    such an item is hidden, private, or otherwise unnameable."""
    index, paths = document["index"], document["paths"]
    names = {int(key): "::".join(value["path"]) for key, value in paths.items()}
    local = {int(key) for key, value in paths.items() if value["crate_id"] == 0}
    exposed = surface(document)[1]
    found = set()
    for identifier in exposed:
        item = index[str(identifier)]
        for part in signatures(index, item):
            for target in referenced(part["inner"], set()):
                if target not in names or (target in local and target not in exposed):
                    found.add(f"{names.get(identifier, identifier)} names {names.get(target, f'item {target}')}")
    return sorted(found)

if __name__ == "__main__":
    arguments = sys.argv[1:]
    check = arguments[:1] == ["--leaks"]
    if len(arguments) != 1 + check:
        sys.exit(__doc__)
    with open(arguments[-1]) as file:
        document = json.load(file)
    if check:
        found = leaks(document)
        print("\n".join(found))
        sys.exit(1 if found else 0)
    print("\n".join(listing(document)))
