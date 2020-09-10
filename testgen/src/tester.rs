use crate::helpers::*;
use crate::tester::TestResult::{Failure, ParseError, ReadError, Success};
use serde::de::DeserializeOwned;
use std::{
    fs::{self, DirEntry},
    io::Write,
    panic::{self, RefUnwindSafe, UnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tempfile::TempDir;

/// A test environment, which is essentially a wrapper around some directory,
/// with some utility functions operating relative to that directory.
#[derive(Debug, Clone)]
pub struct TestEnv {
    /// Directory where the test is being executed
    current_dir: String,
}

impl TestEnv {
    pub fn new(current_dir: &str) -> Option<Self> {
        fs::create_dir_all(current_dir).ok().map(|_| TestEnv {
            current_dir: current_dir.to_string(),
        })
    }

    pub fn push(&self, child: &str) -> Option<Self> {
        let mut path = PathBuf::from(&self.current_dir);
        path.push(child);
        path.to_str().and_then(|path| TestEnv::new(path))
    }

    pub fn current_dir(&self) -> &str {
        &self.current_dir
    }

    pub fn logln(&self, msg: &str) -> Option<()> {
        println!("{}", msg);
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.full_path("log"))
            .ok()
            .and_then(|mut file| writeln!(file, "{}", msg).ok())
    }

    pub fn logln_to(&self, msg: &str, rel_path: impl AsRef<Path>) -> Option<()> {
        println!("{}", msg);
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.full_path(rel_path))
            .ok()
            .and_then(|mut file| writeln!(file, "{}", msg).ok())
    }

    /// Read a file from a path relative to the environment current dir into a string
    pub fn read_file(&self, rel_path: impl AsRef<Path>) -> Option<String> {
        fs::read_to_string(self.full_path(rel_path)).ok()
    }

    /// Write a file to a path relative to the environment current dir
    pub fn write_file(&self, rel_path: impl AsRef<Path>, contents: &str) -> Option<()> {
        fs::write(self.full_path(rel_path), contents).ok()
    }

    /// Parse a file from a path relative to the environment current dir as the given type
    pub fn parse_file<T: DeserializeOwned>(&self, rel_path: impl AsRef<Path>) -> Option<T> {
        self.read_file(rel_path)
            .and_then(|input| serde_json::from_str(&input).ok())
    }

    /// Copy a file from the path outside environment into the environment current dir
    /// Returns None if copying was not successful
    pub fn copy_file_from(&self, path: impl AsRef<Path>) -> Option<()> {
        let path = path.as_ref();
        if !path.is_file() {
            return None;
        }
        let name = path.file_name()?.to_str()?;
        fs::copy(path, self.full_path(name)).ok().map(|_| ())
    }

    /// Copy a file from the path relative to the other environment into the environment current dir
    /// Returns None if copying was not successful
    pub fn copy_file_from_env(&self, other: &TestEnv, path: impl AsRef<Path>) -> Option<()> {
        self.copy_file_from(other.full_path(path))
    }

    /// Convert a relative path to the full path from the test root
    /// Return None if the full path can't be formed
    pub fn full_path(&self, rel_path: impl AsRef<Path>) -> PathBuf {
        PathBuf::from(&self.current_dir).join(rel_path)
    }

    /// Convert a full path to the path relative to the test root
    /// Returns None if the full path doesn't contain test root as prefix
    pub fn rel_path(&self, full_path: impl AsRef<Path>) -> Option<String> {
        match PathBuf::from(full_path.as_ref()).strip_prefix(&self.current_dir) {
            Err(_) => None,
            Ok(rel_path) => match rel_path.to_str() {
                None => None,
                Some(rel_path) => Some(rel_path.to_string()),
            },
        }
    }

    /// Convert a relative path to the full path from the test root, canonicalized
    /// Returns None the full path can't be formed
    pub fn full_canonical_path(&self, rel_path: impl AsRef<Path>) -> Option<String> {
        let full_path = PathBuf::from(&self.current_dir).join(rel_path);
        full_path
            .canonicalize()
            .ok()
            .and_then(|p| p.to_str().map(|x| x.to_string()))
    }
}

#[derive(Debug, Clone)]
pub enum TestResult {
    ReadError,
    ParseError,
    Success,
    Failure { message: String, location: String },
}

/// A function that takes as input the test file path and its content,
/// and returns the result of running the test on it
type TestFn = Box<dyn Fn(&str, &str) -> TestResult>;

/// A function that takes as input the batch file path and its content,
/// and returns the vector of test names/contents for tests in the batch,
/// or None if the batch could not be parsed
type BatchFn = Box<dyn Fn(&str, &str) -> Option<Vec<(String, String)>>>;

pub struct Test {
    /// test name
    pub name: String,
    /// test function
    pub test: TestFn,
}

/// Tester allows you to easily run some test functions over a set of test files.
/// You create a Tester instance with the reference to some specific directory, containing your test files.
/// After a creation, you can add several types of tests there:
///  * add_test() adds a simple test function, which can run on some test, deserilizable from a file.
///  * add_test_with_env() allows your test function to receive several test environments,
///    so that it can easily perform some operations on files when necessary
///  * add_test_batch() adds a batch of test: a function that accepts a ceserializable batch description,
///    and produces a set of test from it
///
///  After you have added all your test functions, you run Tester either on individual files
///  using run_for_file(), or for whole directories, using run_foreach_in_dir();
///  the directories will be traversed recursively top-down.
///
///  The last step involves calling the finalize() function, which will produce the test report
///  and panic in case there was at least one failing test.
///  When there are files in the directories you run Tester on, that could not be read/parsed,
///  it is also considered an error, and leads to panic.
pub struct Tester {
    name: String,
    root_dir: String,
    tests: Vec<Test>,
    batches: Vec<BatchFn>,
    results: std::collections::BTreeMap<String, Vec<(String, TestResult)>>,
}

impl TestResult {
    pub fn is_success(&self) -> bool {
        matches!(self, TestResult::Success)
    }
    pub fn is_failure(&self) -> bool {
        matches!(self, TestResult::Failure {
            message: _,
            location: _,
        })
    }
    pub fn is_readerror(&self) -> bool {
        matches!(self, TestResult::ReadError)
    }
    pub fn is_parseerror(&self) -> bool {
        matches!(self, TestResult::ParseError)
    }
}

impl Tester {
    pub fn new(name: &str, root_dir: &str) -> Tester {
        Tester {
            name: name.to_string(),
            root_dir: root_dir.to_string(),
            tests: vec![],
            batches: vec![],
            results: Default::default(),
        }
    }

    pub fn env(&self) -> Option<TestEnv> {
        TestEnv::new(&self.root_dir)
    }

    pub fn output_env(&self) -> Option<TestEnv> {
        let output_dir = self.root_dir.clone() + "/_" + &self.name;
        fs::create_dir_all(&output_dir)
            .ok()
            .and(TestEnv::new(&output_dir))
    }

    fn capture_test<F>(test: F) -> TestResult
    where
        F: FnOnce() + UnwindSafe,
    {
        let test_result = Arc::new(Mutex::new(ParseError));
        let old_hook = panic::take_hook();
        panic::set_hook({
            let result = test_result.clone();
            Box::new(move |info| {
                let mut result = result.lock().unwrap();
                let message = match info.payload().downcast_ref::<&'static str>() {
                    Some(s) => s.to_string(),
                    None => match info.payload().downcast_ref::<String>() {
                        Some(s) => s.clone(),
                        None => "Unknown error".to_string(),
                    },
                };
                let location = match info.location() {
                    Some(l) => l.to_string(),
                    None => "".to_string(),
                };
                *result = Failure { message, location };
            })
        });
        let result = panic::catch_unwind(|| test());
        panic::set_hook(old_hook);
        match result {
            Ok(_) => Success,
            Err(_) => (*test_result.lock().unwrap()).clone(),
        }
    }

    pub fn add_test<T, F>(&mut self, name: &str, test: F)
    where
        T: 'static + DeserializeOwned + UnwindSafe,
        F: Fn(T) + UnwindSafe + RefUnwindSafe + 'static,
    {
        let test_fn = move |_path: &str, input: &str| match parse_as::<T>(&input) {
            Ok(test_case) => Tester::capture_test(|| {
                test(test_case);
            }),
            Err(_) => ParseError,
        };
        self.tests.push(Test {
            name: name.to_string(),
            test: Box::new(test_fn),
        });
    }

    pub fn add_test_with_env<T, F>(&mut self, name: &str, test: F)
    where
        T: 'static + DeserializeOwned + UnwindSafe,
        F: Fn(T, &TestEnv, &TestEnv, &TestEnv) + UnwindSafe + RefUnwindSafe + 'static,
    {
        let test_env = self.env().unwrap();
        let output_env = self.output_env().unwrap();
        let test_fn = move |path: &str, input: &str| match parse_as::<T>(&input) {
            Ok(test_case) => Tester::capture_test(|| {
                // It is OK to unwrap() here: in case of unwrapping failure, the test will fail.
                let dir = TempDir::new().unwrap();
                let env = TestEnv::new(dir.path().to_str().unwrap()).unwrap();
                let output_dir = output_env.full_path(path);
                let output_env = TestEnv::new(output_dir.to_str().unwrap()).unwrap();
                fs::remove_dir_all(&output_dir).unwrap();
                test(test_case, &env, &test_env, &output_env);
            }),
            Err(_) => ParseError,
        };
        self.tests.push(Test {
            name: name.to_string(),
            test: Box::new(test_fn),
        });
    }

    pub fn add_test_batch<T, F>(&mut self, batch: F)
    where
        T: 'static + DeserializeOwned,
        F: Fn(T) -> Vec<(String, String)> + 'static,
    {
        let batch_fn = move |_path: &str, input: &str| match parse_as::<T>(&input) {
            Ok(test_batch) => Some(batch(test_batch)),
            Err(_) => None,
        };
        self.batches.push(Box::new(batch_fn));
    }

    fn results_for(&mut self, name: &str) -> &mut Vec<(String, TestResult)> {
        self.results
            .entry(name.to_string())
            .or_insert_with(Vec::new)
    }

    fn add_result(&mut self, name: &str, path: &str, result: TestResult) {
        self.results_for(name).push((path.to_string(), result));
    }

    fn read_error(&mut self, path: &str) {
        self.results_for("")
            .push((path.to_string(), TestResult::ReadError))
    }

    fn parse_error(&mut self, path: &str) {
        self.results_for("")
            .push((path.to_string(), TestResult::ParseError))
    }

    pub fn successful_tests(&self, test: &str) -> Vec<String> {
        let mut tests = Vec::new();
        if let Some(results) = self.results.get(test) {
            for (path, res) in results {
                if let Success = res {
                    tests.push(path.clone())
                }
            }
        }
        tests
    }

    pub fn failed_tests(&self, test: &str) -> Vec<(String, String, String)> {
        let mut tests = Vec::new();
        if let Some(results) = self.results.get(test) {
            for (path, res) in results {
                if let Failure { message, location } = res {
                    tests.push((path.clone(), message.clone(), location.clone()))
                }
            }
        }
        tests
    }

    pub fn unreadable_tests(&self) -> Vec<String> {
        let mut tests = Vec::new();
        if let Some(results) = self.results.get("") {
            for (path, res) in results {
                if let ReadError = res {
                    tests.push(path.clone())
                }
            }
        }
        tests
    }

    pub fn unparseable_tests(&self) -> Vec<String> {
        let mut tests = Vec::new();
        if let Some(results) = self.results.get("") {
            for (path, res) in results {
                if let ParseError = res {
                    tests.push(path.clone())
                }
            }
        }
        tests
    }

    fn run_for_input(&mut self, path: &str, input: &str) {
        let mut results = Vec::new();
        for Test { name, test } in &self.tests {
            match test(path, input) {
                TestResult::ParseError => continue,
                res => results.push((name.to_string(), path, res)),
            }
        }
        if !results.is_empty() {
            for (name, path, res) in results {
                self.add_result(&name, path, res)
            }
        } else {
            // parsing as a test failed; try parse as a batch
            let mut res_tests = Vec::new();
            for batch in &self.batches {
                match batch(path, input) {
                    None => continue,
                    Some(tests) => {
                        for (name, input) in tests {
                            let test_path = path.to_string() + "/" + &name;
                            res_tests.push((test_path, input));
                        }
                    }
                }
            }
            if !res_tests.is_empty() {
                for (path, input) in res_tests {
                    self.run_for_input(&path, &input);
                }
            } else {
                // parsing both as a test and as a batch failed
                self.parse_error(path);
            }
        }
    }

    pub fn run_for_file(&mut self, path: &str) {
        match self.env().unwrap().read_file(path) {
            None => self.read_error(path),
            Some(input) => self.run_for_input(path, &input),
        }
    }

    pub fn run_foreach_in_dir(&mut self, dir: &str) {
        let full_dir = PathBuf::from(&self.root_dir).join(dir);
        let starts_with_underscore = |entry: &DirEntry| {
            if let Some(last) = entry.path().iter().rev().next() {
                if let Some(last) = last.to_str() {
                    if last.starts_with('_') {
                        return true;
                    }
                }
            }
            false
        };
        match full_dir.to_str() {
            None => self.read_error(dir),
            Some(full_dir) => match fs::read_dir(full_dir) {
                Err(_) => self.read_error(full_dir),
                Ok(paths) => {
                    for path in paths {
                        if let Ok(entry) = path {
                            // ignore path components starting with '_'
                            if starts_with_underscore(&entry) {
                                continue;
                            }
                            if let Ok(kind) = entry.file_type() {
                                let path = format!("{}", entry.path().display());
                                let rel_path = self.env().unwrap().rel_path(&path).unwrap();
                                if kind.is_file() || kind.is_symlink() {
                                    if !rel_path.ends_with(".json") {
                                        continue;
                                    } else {
                                        self.run_for_file(&rel_path);
                                    }
                                } else if kind.is_dir() {
                                    self.run_foreach_in_dir(&rel_path);
                                }
                            }
                        }
                    }
                }
            },
        }
    }

    pub fn finalize(&mut self) {
        let env = self.output_env().unwrap();
        env.write_file("report", "");
        let print = |msg: &str| {
            env.logln_to(msg, "report");
        };
        let mut do_panic = false;

        print(&format!(
            "\n====== Report for '{}' tester run ======",
            &self.name
        ));
        for name in self.results.keys() {
            if name.is_empty() {
                continue;
            }
            print(&format!("\nResults for '{}'", name));
            let tests = self.successful_tests(name);
            if !tests.is_empty() {
                print("  Successful tests:  ");
                for path in tests {
                    print(&format!("    {}", path));
                    if let Some(logs) = env.read_file(&(path + "/log")) {
                        print(&logs)
                    }
                }
            }
            let tests = self.failed_tests(name);
            if !tests.is_empty() {
                do_panic = true;
                print("  Failed tests:  ");
                for (path, message, location) in tests {
                    print(&format!("    {}, '{}', {}", path, message, location));
                    if let Some(logs) = env.read_file(&(path + "/log")) {
                        print(&logs)
                    }
                }
            }
        }
        let tests = self.unreadable_tests();
        if !tests.is_empty() {
            do_panic = true;
            print("\nUnreadable tests:  ");
            for path in tests {
                print(&format!("  {}", path))
            }
        }
        let tests = self.unparseable_tests();
        if !tests.is_empty() {
            do_panic = true;
            print("\nUnparseable tests:  ");
            for path in tests {
                print(&format!("  {}", path))
            }
        }
        print(&format!(
            "\n====== End of report for '{}' tester run ======\n",
            &self.name
        ));
        if do_panic {
            panic!("Some tests failed or could not be read/parsed");
        }
    }
}