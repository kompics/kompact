use kompact::prelude::*;

fn match_msg(msg: NetMessage) -> () {
	match_deser! {
		msg {
    		msg(_res) [using String] => (), //~ ERROR: Must specify a deserialisation target type
    	}
    };
    match_deser! {
		msg {
    		nsg(_res): String => (), //~ ERROR: Illegal case `nsg`. Allowed values: [`msg`, `err`, `default`]
    	}
    };
    match_deser! {
		msg {
    		my_msg: String => (), //~ ERROR: Illegal `match_deser!` item(s). See docs for example of legal items.  Offending item(s): my_msg : String => ()
    	}
    };
    match_deser! {
		msg {
    		my_msg: String => () //~ ERROR: Illegal `match_deser!` items(s). See docs for example of legal items.  Offending item(s): my_msg : String => ()
    	}
    };
    match_deser!(msg; {
    	my_msg: String => (), //~ ERROR: You are using an old `match_deser!` format. See docs for the new format.
    })
}

fn main() { 
	// unused 
}
