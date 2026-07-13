import { useState } from 'react'
import {
  Snackbar,
  Alert,
  IconButton,
  Dialog,
  DialogTitle,
  DialogContent,
  DialogActions,
  Button,
  Typography,
  Box,
} from '@mui/material'
import { Close as CloseIcon, InfoOutlined } from '@mui/icons-material'

interface ErrorSnackbarProps {
  error: string | null
  onClose: () => void
}

export default function ErrorSnackbar({ error, onClose }: ErrorSnackbarProps) {
  const [dialogOpen, setDialogOpen] = useState(false)

  const handleDialogOpen = () => {
    setDialogOpen(true)
  }

  const handleDialogClose = () => {
    setDialogOpen(false)
  }

  const handleSnackbarClose = () => {
    onClose()
    setDialogOpen(false)
  }

  return (
    <>
      <Snackbar
        open={!!error}
        onClose={handleSnackbarClose}
        anchorOrigin={{ vertical: 'top', horizontal: 'center' }}
      >
        <Alert
          severity="error"
          variant="filled"
          onClose={handleSnackbarClose}
          action={
            <>
              <IconButton
                size="small"
                color="inherit"
                onClick={handleDialogOpen}
                title="查看详情"
              >
                <InfoOutlined fontSize="small" />
              </IconButton>
              <IconButton
                size="small"
                color="inherit"
                onClick={handleSnackbarClose}
              >
                <CloseIcon fontSize="small" />
              </IconButton>
            </>
          }
          sx={{ minWidth: 300 }}
        >
          请求失败
        </Alert>
      </Snackbar>

      <Dialog
        open={dialogOpen}
        onClose={handleDialogClose}
        maxWidth="sm"
        fullWidth
      >
        <DialogTitle>
          <Box display="flex" alignItems="center" gap={1}>
            <InfoOutlined color="error" />
            错误详情
          </Box>
        </DialogTitle>
        <DialogContent>
          <Typography variant="body1" gutterBottom fontWeight="medium">
            错误信息:
          </Typography>
          <Box
            sx={{
              bgcolor: 'action.hover',
              p: 2,
              borderRadius: 1,
              fontFamily: 'monospace',
              fontSize: '0.875rem',
              wordBreak: 'break-word',
              whiteSpace: 'pre-wrap',
              maxHeight: 300,
              overflow: 'auto',
            }}
          >
            {error || '未知错误'}
          </Box>
        </DialogContent>
        <DialogActions>
          <Button onClick={handleDialogClose}>关闭</Button>
        </DialogActions>
      </Dialog>
    </>
  )
}

