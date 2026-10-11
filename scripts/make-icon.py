#!/usr/bin/env python3
"""Generate the app's simple music-note icon without external tooling."""
import pathlib,struct,subprocess,sys,zlib
res=pathlib.Path(sys.argv[1]);iconset=res/'RustMusic.iconset';iconset.mkdir(exist_ok=True)
def chunk(name,data):return struct.pack('>I',len(data))+name+data+struct.pack('>I',zlib.crc32(name+data)&0xffffffff)
def png(size):
 rows=[]
 for y in range(size):
  row=bytearray(b'\0')
  for x in range(size):
   u,v=x/size,y/size
   radius=.2
   inside=(radius<=u<=1-radius or radius<=v<=1-radius or (u-(radius if u<.5 else 1-radius))**2+(v-(radius if v<.5 else 1-radius))**2<radius**2)
   note=((.49<u<.55 and .27<v<.70) or (.72<u<.78 and .22<v<.62) or (.49<u<.78 and .22+(1-u)*.18<v<.31+(1-u)*.18) or ((u-.445)/.108)**2+((v-.71)/.08)**2<1 or ((u-.675)/.108)**2+((v-.63)/.08)**2<1)
   rgba=(248,100+int(35*u),111+int(12*v),255) if note else (25+int(14*v),25+int(7*u),37+int(15*u),255 if inside else 0)
   row.extend(rgba)
  rows.append(bytes(row))
 return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',size,size,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(b''.join(rows),9))+chunk(b'IEND',b'')
for size in [16,32,128,256,512]:
 for factor in [1,2]:(iconset/f'icon_{size}x{size}{"@2x" if factor==2 else ""}.png').write_bytes(png(size*factor))
subprocess.run(['iconutil','-c','icns','-o',str(res/'RustMusic.icns'),str(iconset)],check=True)
