use std::cmp::Ordering;

/// Semantic version for skills: major.minor.patch
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SkillVersion {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn is_outdated_compared_to(&self, latest: &Self) -> bool {
        self < latest
    }

    pub fn is_same_major(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

impl std::fmt::Display for SkillVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for SkillVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Handle "v1.2.3" prefix
        let s = s.trim_start_matches('v');
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("Invalid version format: {s}"));
        }
        let major = parts[0].parse::<u32>().map_err(|e| e.to_string())?;
        let minor = parts[1].parse::<u32>().map_err(|e| e.to_string())?;
        let patch = parts[2].parse::<u32>().map_err(|e| e.to_string())?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl PartialOrd for SkillVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SkillVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parse() {
        let v: SkillVersion = "1.2.3".parse().unwrap();
        assert_eq!(v, SkillVersion::new(1, 2, 3));
    }

    #[test]
    fn test_version_parse_with_prefix() {
        let v: SkillVersion = "v0.5.1".parse().unwrap();
        assert_eq!(v, SkillVersion::new(0, 5, 1));
    }

    #[test]
    fn test_version_ordering() {
        let v1 = SkillVersion::new(0, 5, 0);
        let v2 = SkillVersion::new(0, 5, 1);
        assert!(v1 < v2);
        assert!(v1.is_outdated_compared_to(&v2));
    }

    #[test]
    fn test_version_equal() {
        let v1 = SkillVersion::new(1, 0, 0);
        let v2 = SkillVersion::new(1, 0, 0);
        assert_eq!(v1, v2);
        assert!(!v1.is_outdated_compared_to(&v2));
    }
}
