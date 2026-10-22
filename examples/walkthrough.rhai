// A tour of the script functions against zenith-iec104's example server
// (CA 1, scaled values at 100 to 102 in group 1, a switch at 4001):
//   cargo run -p iec104-tokio --example server -- 127.0.0.1:2404
//   iec104-workbench run examples/walkthrough.rhai --log-level info

connect();

// Group 1 interrogation: walk every object the station sends back.
interrogation(21);
expect(!wait_for(100, 7, 5000).negative, "group interrogation refused");
let reply = wait_for(11, 21, 5000);
log("CA " + reply.ca + " sent " + reply.objects.len() + " " + reply.type_name);
for object in reply.objects {
    log("  IOA " + object.ioa + " = " + object.value + " " + object.quality);
}
expect(reply.objects.len() == 3, "group 1 should hold 3 points");
wait_for(100, 10, 5000);

// Read one point, then the same read as raw octets (type 102, cause 5,
// CA 1, IOA 102 in the default 2-2-3 address sizes).
read(101);
expect(wait_for(11, 5, 5000).objects[0].value == "2000", "IOA 101 should read 2000");
send_hex("66 01 05 00 01 00 66 00 00");
expect(wait_for(11, 5, 5000).objects[0].value == "3000", "IOA 102 should read 3000");

// A direct execute (this server takes no select); the switch then reports
// its new position, time tagged, as return information (type 30, cause 11).
single_command(4001, false, false);
expect(!wait_for(45, 7, 5000).negative, "execute refused");
wait_for(45, 10, 5000);
let position = wait_for(30, 11, 5000).objects[0];
log("IOA 4001 reports " + position.value);
expect(position.value == "OFF", "IOA 4001 should report OFF");

// Point 100 changes every five seconds as a spontaneous event (cause 3).
let event = wait_for(11, 3, 7000);
log("spontaneous IOA 100 = " + event.objects[0].value);

stop_dt();
disconnect();
