" Botwork: comments start with `#`; blocks are indented four spaces.
if exists('b:did_ftplugin')
  finish
endif
let b:did_ftplugin = 1

setlocal commentstring=#\ %s
setlocal comments=:#
setlocal expandtab shiftwidth=4 softtabstop=4
setlocal formatoptions-=t formatoptions+=croql

let b:undo_ftplugin = 'setlocal commentstring< comments< expandtab< shiftwidth< softtabstop< formatoptions<'
