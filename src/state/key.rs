//! Map keys for state: names GitHub compares without case, and commit SHAs.

use std::borrow::{Borrow, Cow};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::hash::{Hash, Hasher};

/// A name GitHub matches case-insensitively, such as a login, a repository, team or label name.
///
/// Keeps the spelling it was created with for display and compares on its lowercase form, which
/// it computes once. Maps keyed by `Name` are looked up with a `&str` through [`NameMap`].
#[derive(Clone)]
pub struct Name {
    display: String,
    folded: String,
}

impl Name {
    pub fn new(display: impl Into<String>) -> Self {
        let display = display.into();
        let folded = fold(&display).into_owned();
        Self { display, folded }
    }

    /// The spelling the name was created with.
    pub fn as_str(&self) -> &str {
        &self.display
    }

    /// The lowercase form names compare on.
    pub fn folded(&self) -> &str {
        &self.folded
    }

    /// Whether `other` names the same thing, ignoring case.
    pub fn matches(&self, other: &str) -> bool {
        self.folded == fold(other)
    }
}

/// The lowercase form of `name` that [`Name`] compares on, borrowed when already lowercase.
pub fn fold(name: &str) -> Cow<'_, str> {
    if name.chars().any(char::is_uppercase) {
        Cow::Owned(name.to_lowercase())
    } else {
        Cow::Borrowed(name)
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.folded == other.folded
    }
}

impl Eq for Name {}

impl Hash for Name {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.folded.hash(state);
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.folded.cmp(&other.folded)
    }
}

/// Borrows the folded form, so a map keyed by `Name` takes `&fold(name)` as a key.
impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.folded
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.display, formatter)
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Name {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Lookups by any spelling of a name in a map keyed by [`Name`].
pub trait NameMap<V> {
    fn get_name(&self, name: &str) -> Option<&V>;
    fn get_name_mut(&mut self, name: &str) -> Option<&mut V>;
    fn remove_name(&mut self, name: &str) -> Option<V>;

    fn contains_name(&self, name: &str) -> bool {
        self.get_name(name).is_some()
    }
}

impl<V> NameMap<V> for HashMap<Name, V> {
    fn get_name(&self, name: &str) -> Option<&V> {
        self.get(fold(name).as_ref())
    }

    fn get_name_mut(&mut self, name: &str) -> Option<&mut V> {
        self.get_mut(fold(name).as_ref())
    }

    fn remove_name(&mut self, name: &str) -> Option<V> {
        self.remove(fold(name).as_ref())
    }
}

impl<V> NameMap<V> for BTreeMap<Name, V> {
    fn get_name(&self, name: &str) -> Option<&V> {
        self.get(fold(name).as_ref())
    }

    fn get_name_mut(&mut self, name: &str) -> Option<&mut V> {
        self.get_mut(fold(name).as_ref())
    }

    fn remove_name(&mut self, name: &str) -> Option<V> {
        self.remove(fold(name).as_ref())
    }
}

/// A full git object id: 40 lowercase hex digits.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sha(String);

/// Text that is not a 40-digit hex object id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSha(pub String);

impl fmt::Display for InvalidSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "not a 40-digit hex SHA: {:?}", self.0)
    }
}

impl std::error::Error for InvalidSha {}

impl Sha {
    /// Parses 40 hex digits in either case; stores them lowercase.
    pub fn parse(text: &str) -> Result<Self, InvalidSha> {
        if text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(Self(text.to_ascii_lowercase()))
        } else {
            Err(InvalidSha(text.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for Sha {
    type Err = InvalidSha;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl TryFrom<String> for Sha {
    type Error = InvalidSha;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<Sha> for String {
    fn from(sha: Sha) -> Self {
        sha.0
    }
}

/// Borrows the hex digits, so a map keyed by `Sha` takes a lowercase `&str`.
impl Borrow<str> for Sha {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Sha {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for Sha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, formatter)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::{Name, NameMap, Sha, fold};

    #[test]
    fn names_compare_without_case_and_keep_their_spelling() {
        let name = Name::new("Octo-Cat");
        assert_eq!(name.as_str(), "Octo-Cat");
        assert_eq!(name.to_string(), "Octo-Cat");
        assert_eq!(name.folded(), "octo-cat");
        assert_eq!(name, Name::new("OCTO-cat"));
        assert!(name.matches("octo-CAT"));
        assert!(!name.matches("octocat"));
        assert!(Name::new("a") < Name::new("B"));
    }

    #[test]
    fn fold_borrows_lowercase_names() {
        assert!(matches!(fold("bug"), std::borrow::Cow::Borrowed("bug")));
        assert_eq!(fold("Bug :Bug:"), "bug :bug:");
        assert_eq!(fold("ÉTÉ"), "été");
    }

    #[test]
    fn maps_look_names_up_in_any_case() {
        let mut hashed = HashMap::new();
        hashed.insert(Name::new("Alice"), 1);
        assert_eq!(hashed.get_name("ALICE"), Some(&1));
        assert!(hashed.contains_name("alice"));
        *hashed.get_name_mut("aLiCe").unwrap() += 1;
        assert_eq!(hashed.remove_name("alice"), Some(2));
        assert!(hashed.is_empty());

        let mut ordered = BTreeMap::new();
        ordered.insert(Name::new("Bob"), ());
        ordered.insert(Name::new("bob"), ());
        assert_eq!(ordered.len(), 1);
        assert_eq!(ordered.keys().next().unwrap().as_str(), "Bob");
        assert!(ordered.contains_name("BOB"));
    }

    #[test]
    fn shas_are_forty_hex_digits_stored_lowercase() {
        let upper = "0123456789ABCDEF0123456789ABCDEF01234567";
        let sha = Sha::parse(upper).unwrap();
        assert_eq!(sha.as_str(), upper.to_ascii_lowercase());
        assert_eq!(sha, upper.parse::<Sha>().unwrap());
        assert!(Sha::parse("abc").is_err());
        assert!(Sha::parse(&"g".repeat(40)).is_err());
        assert!(Sha::try_from("0".repeat(41)).is_err());

        let mut map = HashMap::new();
        map.insert(sha.clone(), 1);
        assert_eq!(map.get(sha.as_str()), Some(&1));
        assert_eq!(String::from(sha), upper.to_ascii_lowercase());
    }
}
