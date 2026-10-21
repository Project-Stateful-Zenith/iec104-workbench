// A smoke test against zenith-iec104's example server (CA 1):
//   cargo run -p iec104-tokio --example server -- 127.0.0.1:2404
//   iec104-workbench run examples/smoke.rhai --log-level all

connect();

interrogation(20);
let con = wait_for(100, 7, 5000);
expect(!con.negative, "interrogation refused");
wait_for(100, 10, 5000);
expect(point(1, 101) == "2000", "IOA 101 should read 2000");

clock_sync();
expect(!wait_for(103, 7, 5000).negative, "clock sync refused");

single_command(4001, true, false);
expect(!wait_for(45, 7, 5000).negative, "command refused");
wait_for(45, 10, 5000);
sleep(200);
expect(point(1, 4001) == "ON", "IOA 4001 should be ON");
log("IOA 4001 is " + point(1, 4001));

disconnect();
