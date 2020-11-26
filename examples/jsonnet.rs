use structopt::StructOpt as _;

#[derive(Debug, structopt::StructOpt)]
struct Opt {
    #[structopt(long = "ext-str")]
    ext_str: Vec<String>,
    #[structopt(long = "ext-code")]
    ext_code: Vec<String>,
    #[structopt(short = "e", long = "exec")]
    exec: bool,
    filename_or_code: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();

    let mut vm = gojsonnet::Vm::default();
    for ext_str in opt.ext_str {
        let mut it = ext_str.splitn(2, '=');
        let key = it.next().unwrap();
        let val = it.next().unwrap();
        vm.ext_var(key, val)?;
    }
    for ext_code in opt.ext_code {
        let mut it = ext_code.splitn(2, '=');
        let key = it.next().unwrap();
        let val = it.next().unwrap();
        vm.ext_code(key, val)?;
    }
    let (code, filename) = if opt.exec {
        (opt.filename_or_code, "<exec>".to_owned())
    } else {
        (
            std::fs::read_to_string(&opt.filename_or_code)?,
            opt.filename_or_code,
        )
    };
    let json: serde_json::Value = vm.evaluate_snippet(&filename, &code)?;
    println!("{}", json);
    Ok(())
}
