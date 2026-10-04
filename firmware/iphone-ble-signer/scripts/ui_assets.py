#!/usr/bin/env python3
"""Native-size idle and request artwork; standard library, no resampling/fonts."""
import struct
from pin_background import ASSETS, convert, png


def save(name, rgb):
    packed = bytearray()
    for i in range(0, len(rgb), 3):
        r,g,b = rgb[i:i+3]
        packed.extend(struct.pack('<H', ((r>>3)<<11)|((g>>2)<<5)|(b>>3)))
    assert len(packed) == 40960
    (ASSETS/(name+'_128x160.rgb565')).write_bytes(packed)
    (ASSETS/(name+'_preview.png')).write_bytes(png(rgb))


def inside(x,y):
    cx=min(max(x+0.5,33),94)
    cy=min(max(y+0.5,14),16)
    return (x+0.5-cx)**2+(y+0.5-cy)**2<=144


if __name__ == '__main__':
    save('idle',convert((ASSETS/'idle_reference.png').read_bytes(),False))
    source=convert((ASSETS/'features_reference.png').read_bytes(),False)
    for selected,name in enumerate(['send','stake','swap','squads']):
        target=bytearray([232,232,232])*128*160
        for row in range(4):
            for y in range(30):
                for x in range(128):
                    start=((13+row*37+y)*128+x)*3
                    color=source[start:start+3]
                    if (row==0)!=(row==selected) and inside(x,y):
                        color=bytes([232,232,232] if sum(color)<384 else [0,0,0])
                    start=((16+row*31+y)*128+x)*3
                    target[start:start+3]=color
        save('feature_'+name,target)
