# Shared configuration size dependency

S2 published `3fc57ec55805c71fbef9d0348d92775d04196e6e` on `tl/cma-log-segments/config-test-split`, from current develop `ce72ee311`. S1 and other slices can normally merge `origin/tl/cma-log-segments/config-test-split`; it carries only this mechanical extraction.

Moves the existing `reconciliation_tests` module from `api/crates/cma/src/config/features.rs` into `features/reconciliation_tests.rs`. Test body and fully qualified name are unchanged. No flag, default, assertion or parser behavior is removed. The parent shrinks by26lines, resolving S1's reported506-line root; the combined root is473lines after normal merge.

Formatting, whitespace and configuration documentation pass. The unchanged test remains in the combined scheduler/config selection. No separate heavy pass or PR is claimed for the extraction. Heavy steps remain separately locked with -k -n -t3600 and two jobs.
