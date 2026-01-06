//! Build script for RISC Zero guest program

fn main() {
    #[cfg(feature = "risc0")]
    {
        risc0_build::embed_methods();
    }

    #[cfg(not(feature = "risc0"))]
    {
        println!("cargo:warning=Building zk-perp-methods without RISC Zero support (mock mode)");
    }
}
