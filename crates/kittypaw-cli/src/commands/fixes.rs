use kittypaw_store::Store;

use super::helpers::db_path;

pub(crate) fn run_fixes_list(skill_id: Option<&str>) {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    let skill = skill_id.unwrap_or("*");
    // If no skill_id, list all distinct skill_ids from recent fixes
    if skill == "*" {
        // Show a summary — query all fixes grouped by skill
        println!("Use: kittypaw fixes list <skill_id>");
        return;
    }

    match store.list_fixes(skill) {
        Ok(fixes) if fixes.is_empty() => println!("No fixes recorded for '{skill}'."),
        Ok(fixes) => {
            println!("=== Fixes for '{}' ===\n", skill);
            for f in &fixes {
                let status = if f.applied { "applied" } else { "pending" };
                println!(
                    "  #{} [{}] {} — {}",
                    f.id,
                    status,
                    f.created_at,
                    f.error_msg.chars().take(80).collect::<String>()
                );
            }
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}

pub(crate) fn run_fixes_show(fix_id: i64) {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    match store.get_fix(fix_id) {
        Ok(Some(f)) => {
            let status = if f.applied { "APPLIED" } else { "PENDING" };
            println!("=== Fix #{} [{}] ===", f.id, status);
            println!("Skill: {}", f.skill_id);
            println!("Error: {}", f.error_msg);
            println!("Date:  {}", f.created_at);
            println!("\n--- Old Code ---");
            println!("{}", f.old_code);
            println!("\n--- New Code ---");
            println!("{}", f.new_code);
        }
        Ok(None) => eprintln!("Fix #{fix_id} not found."),
        Err(e) => eprintln!("Error: {e}"),
    }
}

pub(crate) fn run_fixes_approve(fix_id: i64) {
    let db_path = db_path();
    let store = match Store::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Database error: {e}");
            return;
        }
    };

    match store.apply_fix(fix_id) {
        Ok(true) => println!("Fix #{fix_id} approved and applied."),
        Ok(false) => eprintln!("Fix #{fix_id} not found or already applied."),
        Err(e) => eprintln!("Error: {e}"),
    }
}
