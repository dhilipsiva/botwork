import runpy
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
API = runpy.run_path(ROOT / "scripts/public_api.py")


def item(identifier, name, inner, visibility="public", attrs=()):
    return {"id": identifier, "crate_id": 0, "name": name, "visibility": visibility,
            "attrs": list(attrs), "inner": inner}


def path(name, identifier):
    return {"resolved_path": {"path": name, "id": identifier, "args": None}}


def function(*types):
    return {"function": {"sig": {"inputs": [["value", kind] for kind in types], "output": None}}}


def document(signature=None):
    """A crate with a module holding a struct, an enum, a function, a private
    function, and a re-export of the struct; rustdoc leaves hidden items out.

    `signature` gives the types the function takes: by default the struct.
    """
    index = {
        "0": item(0, "krate", {"module": {"items": [1]}}),
        "1": item(1, "api", {"module": {"items": [2, 6, 9, 10, 11]}}),
        "2": item(2, "Options", {"struct": {"kind": {"plain": {"fields": [3, 4]}}, "impls": [5, 12, 13]}},
                  attrs=["non_exhaustive"]),
        "3": item(3, "limit", {"struct_field": path("usize", 90)}),
        "4": item(4, "secret", {"struct_field": path("usize", 90)}, visibility="default"),
        "5": item(5, None, {"impl": {"trait": None, "items": [14, 15], "is_synthetic": False, "blanket_impl": None}}),
        "6": item(6, "Kind", {"enum": {"variants": [7, 8], "impls": []}}),
        "7": item(7, "One", {"variant": {"kind": "plain"}}),
        "8": item(8, "Two", {"variant": {"kind": "plain"}}),
        "9": item(9, "run", function(*(signature or [path("Options", 2)]))),
        "10": item(10, "helper", function(), visibility="crate"),
        "11": item(11, None, {"use": {"source": "self::Options", "name": "Settings", "id": 2, "is_glob": False}}),
        "12": item(12, None, {"impl": {"trait": {"path": "Clone", "id": 91}, "items": [], "is_synthetic": False, "blanket_impl": None}}),
        "13": item(13, None, {"impl": {"trait": {"path": "Send", "id": 92}, "items": [], "is_synthetic": True, "blanket_impl": None}}),
        "14": item(14, "new", function()),
        "15": item(15, "check", function(), visibility="crate"),
    }
    paths = {str(key): {"crate_id": 0, "path": value} for key, value in
             {0: ["krate"], 1: ["krate", "api"], 2: ["krate", "api", "Options"], 6: ["krate", "api", "Kind"],
              9: ["krate", "api", "run"], 50: ["krate", "api", "Hidden"]}.items()}
    paths["90"] = {"crate_id": 1, "path": ["core", "primitive", "usize"]}
    return {"root": 0, "index": index, "paths": paths}


class Listing(unittest.TestCase):
    def test_public_items_their_members_and_every_public_path_are_listed(self):
        self.assertEqual(API["listing"](document()), sorted([
            "mod krate::api",
            "struct krate::api::Options #[non_exhaustive]",
            "struct krate::api::Settings #[non_exhaustive]",
            "field krate::api::Options::limit",
            "field krate::api::Settings::limit",
            "fn krate::api::Options::new",
            "fn krate::api::Settings::new",
            "impl Clone for krate::api::Options",
            "impl Clone for krate::api::Settings",
            "enum krate::api::Kind",
            "variant krate::api::Kind::One",
            "variant krate::api::Kind::Two",
            "fn krate::api::run",
        ]))

    def test_private_members_and_synthetic_impls_are_left_out(self):
        listed = "\n".join(API["listing"](document()))
        for absent in ("secret", "helper", "check", "Send"):
            with self.subTest(absent=absent):
                self.assertNotIn(absent, listed)


class Leaks(unittest.TestCase):
    def test_signatures_naming_listed_or_foreign_items_do_not_leak(self):
        self.assertEqual(API["leaks"](document()), [])
        self.assertEqual(API["leaks"](document([path("usize", 90)])), [])

    def test_private_fields_and_methods_cannot_leak(self):
        crate = document()
        crate["index"]["4"]["inner"]["struct_field"] = path("Hidden", 77)
        crate["index"]["15"]["inner"] = function(path("Hidden", 77))
        self.assertEqual(API["leaks"](crate), [])

    def test_signatures_naming_hidden_or_unlisted_items_leak(self):
        # Rustdoc leaves a hidden item out entirely; a private one keeps a path.
        for target in (77, 50):
            with self.subTest(target=target):
                found = API["leaks"](document([path("Hidden", target)]))
                self.assertEqual(len(found), 1)
                self.assertTrue(found[0].startswith("krate::api::run names "), found)


class Snapshot(unittest.TestCase):
    def test_the_checked_in_listing_is_sorted_and_free_of_internals(self):
        lines = (ROOT / "docs/public-api.txt").read_text().splitlines()
        self.assertEqual(lines, sorted(set(lines)))
        listed = "\n".join(lines)
        for internal in ("core::language", "BWParser", "PRATT_PARSER", "StatementKind",
                         "execute_statement", "core::ast::suite", "Program::statements"):
            with self.subTest(internal=internal):
                self.assertNotIn(internal, listed)


if __name__ == "__main__":
    unittest.main()
