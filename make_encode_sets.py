# Copyright 2013-2014 Simon Sapin.
#
# Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
# http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
# <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
# option. This file may not be copied, modified, or distributed
# except according to those terms.


# Run as: python make_encode_sets.py > src/encode_sets.rs


print('''\
// Copyright 2013-2014 Simon Sapin.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// Generated by make_encode_sets.py
''')
for name, encoded in [
    ('SIMPLE',   ''),
    ('QUERY',    r''' "#<>'''),
    ('DEFAULT',  r''' "#<>`?{}'''),
    ('USERINFO', r''' "#<>`?{}@'''),
    ('PASSWORD', r''' "#<>`?{}@\/'''),
    ('USERNAME', r''' "#<>`?{}@\/:'''),
    ('FORM_URLENCODED', r''' !"#$%&\'()+,/:;<=>?@[\]^`{|}~'''),
    ('HTTP_VALUE', r''' "%'()*,/:;<->?[\]{}'''),
    ('UNRESERVED', r''' !"#$%&\'*()+,/:;<=>?@[\]^`{|}'''),
]:
    print(
        "pub static %s: [&'static str; 256] = [\n%s\n];\n\n"
        % (name, '\n'.join(
            '   ' + ' '.join(
                '"%s%s",' % ("\\" if chr(b) in '\\"' else "", chr(b))
                if 0x20 <= b <= 0x7E and chr(b) not in encoded
                else '"%%%02X",' % b
                for b in range(s, s + 8)
            ) for s in range(0, 256, 8))))
