use super::super::Result;
use super::catalog::{self, Query};
use std::time::Duration;

pub(super) enum Operation {
    Catalog,
    Preflight(Vec<Query>),
    Inspect {
        queries: Vec<Query>,
        verify: bool,
        legacy: bool,
    },
    Build {
        query: Query,
        checksum: String,
        image: String,
        timeout: u64,
    },
    Abort {
        query: Query,
        checksum: String,
        image: String,
        fence: i64,
    },
    Status(Query),
}
impl Operation {
    pub fn deadline(&self) -> Duration {
        Duration::from_secs(match self {
            Self::Build { timeout, .. } => timeout + 60,
            Self::Inspect { queries, .. } => 30 * queries.len() as u64,
            _ => 45,
        })
    }
}

pub(super) fn parse(arguments: &[String]) -> Result<Operation> {
    let args = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    match args.as_slice() {
        ["--index-catalog"] => Ok(Operation::Catalog),
        ["--index-preflight", names] => Ok(Operation::Preflight(catalog::select(names)?)),
        ["--activity-index-size"] => Ok(Operation::Inspect {
            queries: vec![catalog::activity()],
            verify: false,
            legacy: true,
        }),
        [mode @ ("--index-size" | "--index-verify"), names] => Ok(Operation::Inspect {
            queries: catalog::select(names)?,
            verify: *mode == "--index-verify",
            legacy: false,
        }),
        ["--index-status", name] => Ok(Operation::Status(catalog::one(name)?)),
        ["--activity-index-build", checksum, image] => {
            build("activity-first-page", checksum, image, "1800")
        }
        ["--activity-index-build", checksum, image, timeout] => {
            build("activity-first-page", checksum, image, timeout)
        }
        ["--index-build", name, checksum, image] => build(name, checksum, image, "1800"),
        ["--index-build", name, checksum, image, timeout] => build(name, checksum, image, timeout),
        ["--index-abort", name, checksum, image, fence] => {
            let fence = fence
                .parse::<i64>()
                .map_err(|_| "abort requires the previous attempt fence")?;
            if fence < 1 || fence == i64::MAX {
                return Err("abort fence is outside bounds");
            }
            Ok(Operation::Abort {
                query: catalog::one(name)?,
                checksum: (*checksum).into(),
                image: (*image).into(),
                fence,
            })
        }
        _ => Err("unsupported named-query index command or arguments"),
    }
}
fn build(name: &str, checksum: &str, image: &str, timeout: &str) -> Result<Operation> {
    let timeout = timeout
        .parse::<u64>()
        .map_err(|_| "index timeout must be an integer")?;
    if !(60..=1800).contains(&timeout) {
        return Err("index timeout must be between 60 and 1800 seconds");
    }
    Ok(Operation::Build {
        query: catalog::one(name)?,
        checksum: checksum.into(),
        image: image.into(),
        timeout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).into()).collect()
    }
    #[test]
    fn names_mutation_scope_and_deadlines_are_bounded() {
        assert!(
            matches!(parse(&args(&["--index-preflight", "activity-first-page,agents-owner-page"])),
            Ok(Operation::Preflight(queries)) if queries.len() == 2)
        );
        assert!(matches!(
            parse(&args(&[
                "--index-build",
                "activity-first-page",
                "sum",
                "image",
                "1800"
            ])),
            Ok(Operation::Build { timeout: 1800, .. })
        ));
        assert!(matches!(
            parse(&args(&["--activity-index-size"])),
            Ok(Operation::Inspect { legacy: true, .. })
        ));
        assert!(matches!(
            parse(&args(&["--index-verify", "activity-first-page"])),
            Ok(Operation::Inspect { verify: true, .. })
        ));
        assert_eq!(
            parse(&args(&[
                "--index-build",
                "activity-first-page",
                "sum",
                "image"
            ]))
            .ok()
            .unwrap()
            .deadline(),
            Duration::from_secs(1860)
        );
        for query in catalog::all().into_iter().filter(|q| q.step.is_none()) {
            assert!(parse(&args(&["--index-size", query.name])).is_ok());
            assert!(parse(&args(&["--index-status", query.name])).is_err());
            assert!(parse(&args(&["--index-build", query.name, "sum", "image"])).is_err());
            assert!(parse(&args(&["--index-abort", query.name, "sum", "image", "1"])).is_err());
        }
        for invalid in [
            vec!["--index-size", "unknown"],
            vec!["--index-build", "activity-first-page", "sum", "image", "59"],
            vec![
                "--index-build",
                "activity-first-page",
                "sum",
                "image",
                "1801",
            ],
            vec!["--index-abort", "activity-first-page", "sum", "image", "0"],
            vec!["--index-abort", "activity-first-page", "sum", "image"],
            vec!["--index-size", "activity-first-page", "extra"],
        ] {
            assert!(parse(&args(&invalid)).is_err());
        }
    }
}
