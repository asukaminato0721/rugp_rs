# Format research and attribution

RIO/ICI decryption, serialized class names, and address transforms were ported
from [GARbro ArcRIO.cs](https://github.com/morkt/GARbro/blob/master/ArcFormats/rUGP/ArcRIO.cs).
CRip007 decoding and quantization tables are ported from
[GARbro ImageRIP.cs](https://github.com/morkt/GARbro/blob/master/ArcFormats/rUGP/ImageRIP.cs).

Copyright (C) 2016 by morkt

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to
deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or
sell copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
IN THE SOFTWARE.

Additional format research:
[osmium76/rugptools](https://github.com/osmium76/rugptools), especially the
object prefix, object-reference cache, and CRsa serialization notes. Its
hardcoded Muv-Luv archive position is not used here.

[AgeReVerse's format research](https://github.com/RepressedAlliance/AgeReVerse/tree/main/rUGP/formats)
provided cross-checks for native descriptor field kinds, typed VM literals,
Cr6Ti channel order, and CRip008 predictor and surface conventions. Its newer
Photon archive profiles are not substituted for the 2008 disc's headers.
