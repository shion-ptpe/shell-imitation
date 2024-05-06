pub type DynError = Box<dyn std::error::Error + Send + Sync + 'static>;
use std::io::{self, Write};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use nix::libc;
use nix::unistd::{execvp, fork, ForkResult, dup2};
use std::ffi::CString;
use std::time::Duration;
use std::path::PathBuf;
use std::process::exit;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;

#[derive(Debug)]
enum WorkerMsg {
    Cmd(String),
}

#[derive(Debug)]
struct Worker { }


fn main() {
    loop {
        let (worker_tx, worker_rx) = channel();
        Worker::new().spawn(worker_rx);

        print!("$ ");
        io::stdout().flush().expect("Failed to flush stdout");
        let mut input = String::new();

        match io::stdin().read_line(&mut input) {
            Ok(_) => {
                let input_trimed = input.trim().to_string();
                if input_trimed.is_empty() {
                    continue;
                }
                worker_tx.send(WorkerMsg::Cmd(input_trimed)).unwrap();
                thread::sleep(Duration::from_millis(50));  // $がコマンド結果の後に表示されるように一時的に処置
            }
            Err(error) => println!("error: {error}"),
        }
    }
}


impl Worker {
    fn new() -> Self {
        Worker {
        }
    }
    fn spawn(mut self, worker_rx: Receiver<WorkerMsg>) {
        // 新しいスレッドを生成
        thread::spawn(move || {
            // ワーカースレッドが受信したメッセージを処理
            for msg in worker_rx.iter() {
                match msg {
                    WorkerMsg::Cmd(line) => {
                        match parse_cmd(&line) {
                            // コマンドを解析してプログラムと引数に分割
                            Ok((program, mut args)) => {
                                // ビルトインコマンドであれば実行しない
                                if self.built_in_cmd(program, &args) {
                                    continue;
                                }
                                // 新しいプロセスをフォーク
                                match unsafe{fork()} {
                                    Ok(ForkResult::Parent { .. }) => {
                                        continue;  // 新しいコマンドを受け付けるためにループを継続
                                    }
                                    Ok(ForkResult::Child) => {
                                        // リダイレクトが含まれていた場合は入出力先を変更
                                        self.redirect_cmd(&mut args);

                                        match self.cmd_exec(program, args) {
                                            Err(_) => {
                                                eprintln!("unknown command");
                                                exit(1);
                                            }
                                            Ok(_) => unreachable!(),
                                        }
                                    } 
                                    Err(_) => eprintln!("fork failure"),
                                }
                            }
                            Err(e) => {
                                eprintln!("MySh: {e}");
                            }
                        }
                    }
                }
            }
        });
    }

    fn cmd_exec(&mut self, program: &str, args: Vec<&str>) -> Result<(), DynError> {
        let program = CString::new(program).unwrap();
        let args: Vec<CString> = args.iter().map(|s| CString::new(*s).unwrap()).collect();
        
        // execvpを使ってプログラムを実行
        match execvp(&program, &args) {
            Err(_) => {
                eprintln!("unknown command");
                exit(1);
            }
            Ok(_) => unreachable!(),
        }
    }
    fn built_in_cmd(&mut self, program: &str, args: &Vec<&str>) -> bool {
        match program {
            "exit" => self.run_exit(args),
            "jobs" => {
                eprintln!("jobs command is currently not compatible with MySh");
                true
            }
            "fg" => {
                eprintln!("fg command is currently not compatible with MySh");
                true
            },
            "cd" => self.run_cd(args),
            _ => false,
        }
    }
    fn redirect_cmd(&mut self, args: &mut Vec<&str>){
        // リダイレクト記号を探す
        let index = match args.iter().position(|&x| matches!(x, ">" | "<" | ">>" | "<<")) {
            Some(i) => i,
            None => return,
        }; 
        let filename: &str = args[index+1];  // ファイル名を取得
        let newcmd: Vec<&str> =  args[..index].to_vec();  // リダイレクト記号より前のコマンド部分を取得

        match args[index] {
            ">>" => {
                // すでにファイルが存在したら追記を行う
                let file = match OpenOptions::new().write(true).append(true).open(filename) {
                    Ok(file) => file,
                    // ファイルが存在しなければ新しく作成
                    Err(_) => OpenOptions::new().write(true).create(true).open(filename).unwrap()
                };
                // ファイルディスクリプタを取得
                let fd = file.as_raw_fd();
                // 現在の出力先を取得したファイルディスクリプタに繋げる
                dup2(fd, libc::STDOUT_FILENO).unwrap();
            }
            ">" => {
                // 新しくファイルを作成して書き込み
                let file = OpenOptions::new().write(true).create(true).open(filename).unwrap();
                let fd = file.as_raw_fd();
                dup2(fd, libc::STDOUT_FILENO).unwrap();
            }
            "<" => {
                if let Ok(file) = OpenOptions::new().read(true).open(filename) {
                    let fd = file.as_raw_fd();
                    // 現在の入力元をファイルディスクリプタに繋げる
                    dup2(fd, libc::STDIN_FILENO).unwrap();
                } else {
                    println!("MySh: no such file or directory: {}", filename);
                };
            }
            _ => {}
        };
        // リダイレクト処理部分を除いたコマンドを返す
        *args = newcmd;
    }
    fn run_cd(&mut self, args: &Vec<&str>) -> bool {
        let path = if args.len() == 1 {
            dirs::home_dir()  // ホームディレクトリを取得
                .or_else(|| Some(PathBuf::from("/")))  // ホームディレクトリがない場合はルートディレクトリを使用
                .unwrap()
        } else {
            PathBuf::from(args[1])  // 引数の2番目の要素をPathBufに変換
        };
        // 現在のディレクトリを変更する
        if let Err(e) = std::env::set_current_dir(&path) {
            eprintln!("cd failed: {e}");
            false
        } else {
            true
        }
    }
    fn run_exit(&mut self, args: &Vec<&str>) -> bool {
        if let Some(s) = args.get(1) {
            if let Ok(n) = s.parse::<i32>() {
                // 引数に指定された終了コードを使用
                exit(n)
            } else {
                // 現在のディレクトリを変更する
                eprintln!("{s} is an invalid value");
                return true
            }
        } else {
            exit(0)
        }
    }
}


type CmdResult<'a> = Result<(&'a str, Vec<&'a str>), DynError>;

// execvp -> exec::execvp("echo", &["echo", "foo"]);
fn parse_cmd(line: &str) -> CmdResult {
    let (filename, args) = parse_cmd_one(line)?;
    Ok((filename, args))
}

fn parse_cmd_one(line: &str) -> Result<(&str, Vec<&str>), DynError> {
    let cmd: Vec<&str> = line.split(' ').collect();  // スペースで分割してパース処理
    let mut filename = "";
    let mut args = Vec::new();
    for (n, s) in cmd.iter().filter(|s| !s.is_empty()).enumerate() {
        if n == 0 {
            filename = *s;
        }
        args.push(*s);
    }
    if filename.is_empty() {
        Err("command is empty".into())
    } else {
        Ok((filename, args))
    }
}
