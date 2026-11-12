"""Rasterize the code-native Tidemark mark into Windows icon sizes."""
from PIL import Image, ImageDraw
from pathlib import Path
root = Path(__file__).resolve().parents[1]
im = Image.new('RGBA', (1024, 1024))
d = ImageDraw.Draw(im)
d.rounded_rectangle((0, 0, 1023, 1023), radius=256, fill='#292d2c')
d.rounded_rectangle((160, 400, 672, 624), radius=112, fill='#f7f5f0')
d.ellipse((704, 432, 864, 592), fill='#cf775c')
im.resize((256, 256), Image.Resampling.LANCZOS).save(root / 'assets/brand/tidemark.png')
im.save(root / 'assets/brand/tidemark.ico', sizes=[(16,16),(20,20),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
