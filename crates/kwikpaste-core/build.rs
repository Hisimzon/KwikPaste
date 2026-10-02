fn main() {
    // sqlx::migrate! 只跟踪已有的迁移文件；新增迁移文件时也要重新编译。
    println!("cargo:rerun-if-changed=migrations");
}
