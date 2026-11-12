"""Capture the native settings window without capturing other desktop content."""
import ctypes as c
from ctypes import wintypes as w
import sys
from PIL import Image

u, g = c.windll.user32, c.windll.gdi32
u.FindWindowW.restype = w.HWND
u.GetDC.restype = w.HDC
g.CreateCompatibleDC.restype = w.HDC
g.CreateCompatibleBitmap.restype = w.HBITMAP
g.SelectObject.restype = w.HANDLE
for f, args in [(u.GetWindowRect, [w.HWND, c.POINTER(w.RECT)]),
                (u.GetDC, [w.HWND]), (u.PrintWindow, [w.HWND, w.HDC, w.UINT]),
                (u.ReleaseDC, [w.HWND, w.HDC]),
                (g.CreateCompatibleDC, [w.HDC]),
                (g.CreateCompatibleBitmap, [w.HDC, c.c_int, c.c_int]),
                (g.SelectObject, [w.HDC, w.HANDLE]),
                (g.DeleteObject, [w.HANDLE]), (g.DeleteDC, [w.HDC])]:
    f.argtypes = args
u.SetProcessDPIAware()
hwnd = u.FindWindowW('WinIslandSettings', None)
if not hwnd:
    raise SystemExit('Settings window not found')
rect = w.RECT()
u.GetWindowRect(hwnd, c.byref(rect))
width, height = rect.right - rect.left, rect.bottom - rect.top
dc = u.GetDC(hwnd)
mem = g.CreateCompatibleDC(dc)
bmp = g.CreateCompatibleBitmap(dc, width, height)
old = g.SelectObject(mem, bmp)
assert u.PrintWindow(hwnd, mem, 2), 'PrintWindow failed'
class Header(c.Structure):
    _fields_ = [('size', w.DWORD), ('width', w.LONG), ('height', w.LONG),
                ('planes', w.WORD), ('bits', w.WORD), ('compression', w.DWORD),
                ('image_size', w.DWORD), ('x', w.LONG), ('y', w.LONG),
                ('used', w.DWORD), ('important', w.DWORD)]
header = Header(40, width, -height, 1, 32, 0, 0, 0, 0, 0, 0)
data = c.create_string_buffer(width * height * 4)
g.SelectObject(mem, old)
g.GetDIBits.argtypes = [w.HDC, w.HBITMAP, w.UINT, w.UINT, c.c_void_p, c.c_void_p, w.UINT]
assert g.GetDIBits(mem, bmp, 0, height, data, c.byref(header), 0)
Image.frombuffer('RGB', (width, height), data, 'raw', 'BGRX', 0, 1).save(sys.argv[1])
g.DeleteObject(bmp)
g.DeleteDC(mem)
u.ReleaseDC(hwnd, dc)
print(sys.argv[1])
