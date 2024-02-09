use crate::api::execute_script::EmailAttribute;
use sqlx::FromRow;

#[derive(FromRow, Debug, Clone)]
pub struct Email {
    pub id: String,
    pub html: String,
    pub user: String,
    pub registered: i64,
    pub from_addr: String,
    pub to_addr: String,
    pub subject: String,
}
impl Email {
    pub(crate) fn get_attribute(&self, attribute: EmailAttribute) -> &str {
        match attribute {
            EmailAttribute::Id => &self.id,
            EmailAttribute::FromAddress => &self.from_addr,
            EmailAttribute::Subject => &self.subject,
            EmailAttribute::ToAddress => &self.to_addr,
        }
    }
}
